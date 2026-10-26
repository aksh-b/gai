# gai

A self-hostable agentic AI platform in Rust. You define agents in YAML or in Rust. They can use any model provider, call tools and MCP servers, browse the web, run code in a sandbox, remember things, search your documents and delegate to each other. You run them from a CLI, an HTTP API (including an OpenAI-compatible one), a web dashboard, or your own Rust program. It ships as one binary with one SQLite file, or runs on Postgres.

Website: https://aksh-b.github.io/gai

> Status: early and moving fast (v0.1, unreleased). Phases 1–14 are built, and phase 15 is under way (see [ROADMAP.md](ROADMAP.md)). See [STATUS.md](STATUS.md) for details and known gaps, and [CHANGELOG.md](CHANGELOG.md) for what is coming.

## Features

- **Any model.** Dialects for Anthropic, OpenAI-compatible APIs and ElevenLabs, plus presets for OpenAI, Gemini, Groq, OpenRouter, DeepSeek, Mistral, Together, Fireworks, xAI, Ollama, LM Studio and vLLM. Models are addressed as `provider/model`. Retry, fallback and cost metering are built in, and `router/<name>` picks the cheapest capable model per request.
- **Agents.** A tool loop with parallel tool calls, approval policies, budgets, context compaction, and durable runs that resume after a restart.
- **Personas.** Reusable, versioned voices (tone, style rules, examples, avatar and greeting) that any agent or chat can speak in.
- **Tools and MCP.** Built-ins (calculator, time, files, HTTP fetch, shell), an MCP client (stdio and streamable HTTP, with OAuth sign-in), and your own tools written as Rust functions with `#[gai::tool]`.
- **Code sandbox.** `code_exec` runs Python, JavaScript or bash in a fresh Docker or Podman container with no network and capped resources. Files it writes, such as charts, show up in chat.
- **Browser.** Headless Chrome tools to open pages, read them, click, type and fill in forms, extract data and take screenshots. Domain allow and deny lists apply, private addresses are blocked, and form submits need approval.
- **Memory and RAG.** Self-editing memory blocks, archival memory, and knowledge collections with hybrid search (vector + FTS5, fused with RRF) over text, Markdown, HTML and PDF.
- **Agent Skills.** `SKILL.md` folders or uploaded skills, loaded on demand.
- **Multi-agent.** Delegation through `ask_<agent>` tools, and YAML DAG workflows with parallel nodes, `map` fan-out, `if`, retries, human approval steps and checkpoints.
- **Safety and governance.** Personal-data masking, prompt-injection checks on tool results, topic and output guardrails, an audit log of every change, retention, and deleting a person's data on request.
- **Evaluations.** Eval sets with rule, LLM-judge, grounding and citation scorers; benchmark models, compare versions and gate CI with `gai eval run`. Thumbs up/down on replies, and disliked replies become test cases.
- **Versioning and replay.** Every save of an agent, workflow or persona is a version (diff, label, roll back). Any run can be forked from a step with a different model, prompt or tools, then compared side by side.
- **Bundles.** Share agents and everything they use (personas, skills, workflows, knowledge, MCP servers) as a `.gaibundle` file. Bundles never contain keys.
- **Media.** Speech-to-text, text-to-speech and image generation as tools and endpoints.
- **Triggers.** Cron schedules, HMAC-signed webhooks and file watchers.
- **Interop.** gai runs as an MCP server (`/mcp` or stdio) and an A2A server and client.
- **Server.** REST + SSE API, API keys with scopes, per-key budgets and rate limits, an OpenAI-compatible `/openai/v1`, and OpenAPI 3.1 with interactive docs at `/docs`.
- **Dashboard.** React UI built into the binary: playground, runs, approvals, assistants, personas, workflows (with a live graph), knowledge, memory, skills, triggers, media, models, tools, usage and keys.

## Quick start

Requirements: Rust 1.91+ and, for the dashboard, Node 20+. On Linux you also need a C compiler (for SQLite) and `pkg-config`.

```bash
git clone https://github.com/aksh-b/gai.git
cd gai
cargo build -p gai-cli
```

Everything works offline with the built-in `mock/echo` model. It repeats what you say, or calls a tool when the input is `call:<tool> <json>`:

```bash
cargo run -p gai-cli -- run -a examples/agents/calc.yaml "call:calculator {\"expression\": \"6*7\"}"
cargo run -p gai-cli -- workflow run examples/workflows/hello.yaml -i names='["ada", "alan"]'
```

To use a real model, set the provider's key and pick a model:

```bash
export ANTHROPIC_API_KEY=...
cargo run -p gai-cli -- chat -m anthropic/claude-sonnet-5-5
```

Local models work through [Ollama](https://ollama.com): `ollama pull qwen3:4b`, then use `-m ollama/qwen3:4b`.

### Server and dashboard

Build the dashboard first so it gets compiled into the binary:

```bash
cd web && npm install && npm run build && cd ..
cargo run -p gai-cli -- serve
```

Open http://localhost:8484 for the dashboard and http://localhost:8484/docs for the API reference. On a local server the dashboard signs itself in, and a first-run screen helps you connect a model. A binary built without `web/dist` still serves the API, and its `/` page explains how to add the dashboard.

Auth is open while no API key exists and the server is local. Once you create a key, every request needs one:

```bash
cargo run -p gai-cli -- keys create --name me --scope admin
```

### Docker

```bash
docker build -t gai .
docker run -d --name gai -p 127.0.0.1:8484:8484 -v gai-data:/data \
  -e ANTHROPIC_API_KEY gai serve --local
```

`--local` tells gai that the published port is only reachable from this machine, so the dashboard can sign itself in. Without it (on a public address), create a key first with `docker exec gai gai keys create --name me --scope admin`. Add `--build-arg BROWSER=1` to the build for the browser tools. The code sandbox needs the engine's socket: `-v /var/run/docker.sock:/var/run/docker.sock`. See [docs/deploy.md](docs/deploy.md).

## Defining an agent

```yaml
model: anthropic/claude-sonnet-5-5
description: Researches a topic and writes a short brief.
system: You are {{ agent }}. Today is {{ date }}.
persona: concise-engineer
tools: [http_fetch, browser_*, rag_search]
knowledge: [handbook]
delegates: [writer]
limits:
  max_steps: 20
```

Save it with `gai agents save researcher.yaml`, or create it in the dashboard. See [examples/agents](examples/agents), [examples/workflows](examples/workflows) and [examples/personas](examples/personas), and the [agent guide](docs/agents.md).

## Using gai from Rust

```rust
use gai::agent::AgentSpec;

/// Look up a word in the team glossary.
#[gai::tool]
async fn glossary(word: String) -> String {
    format!("{word}: a term we use a lot")
}

#[tokio::main]
async fn main() -> Result<(), gai::Error> {
    let gai = gai::Gai::builder()
        .default_model("anthropic/claude-sonnet-5-5")
        .tool(glossary_tool())
        .build()?;
    let agent = AgentSpec::new("helper").tools(["glossary", "calculator"]);
    let reply = gai.agent_spec(agent).run("What does 'rollout' mean here?").await?;
    println!("{}", reply.text);
    Ok(())
}
```

`gai.agent("researcher")` runs a stored agent, `.stream(..)` follows its events, and `gai::client::Client` talks to a running server over HTTP. Run the examples with `cargo run -p gai --example quickstart` (also `custom_tool` and `client`). See the [SDK guide](docs/sdk.md).

## Configuration

gai reads the first of `$GAI_CONFIG`, `./gai.toml` and `~/.config/gai/gai.toml`. Start from [gai.example.toml](gai.example.toml): copy it to `gai.toml` (git ignores that file, so your settings stay yours). Any value can be overridden with `GAI_*` environment variables, using `__` between nested keys (for example `GAI_DEFAULT_MODEL=ollama/qwen3:4b` or `GAI_PROVIDERS__OLLAMA__BASE_URL=http://gpu-box:11434/v1`). Every option is documented in [crates/gai/src/config.rs](crates/gai/src/config.rs).

```toml
default_model = "router/auto"

# A local model from Ollama
[[providers.ollama.models]]
id = "qwen3:4b"
capabilities = { tools = true, context_window = 32768 }

# router/auto: easy chat goes local, harder work goes to a bigger model
[routers.auto]
models = [
  { model = "ollama/qwen3:4b", level = "easy" },
  { model = "anthropic/claude-sonnet-5-5", level = "hard" },
]

[sandbox]
backend = "container"   # or "local" (no isolation) or "off"

[browser]
deny_domains = ["facebook.com"]
```

Settings chosen in the dashboard (default model, provider keys, pulled Ollama models) are stored in the database and win over `gai.toml`.

## CLI

| Command | What it does |
|---|---|
| `gai serve` | HTTP API and dashboard (`--ui <dir>` to serve a dashboard build from disk, `--no-ui` for the API only) |
| `gai run` / `gai chat` / `gai ask` | One-shot run, terminal chat UI, quick question |
| `gai agents`, `gai workflow` | Manage, version, diff and roll back agents and workflows |
| `gai runs`, `gai sessions`, `gai replay` | Inspect and resume runs; fork a run from any step |
| `gai ingest`, `gai collections`, `gai memory`, `gai skills` | Knowledge, memory and skills |
| `gai eval` | Run eval sets, benchmark models, compare and gate CI |
| `gai audit`, `gai retention`, `gai forget-user` | Who changed what; delete old conversations; delete a person's data |
| `gai bundle` | Export, inspect and import `.gaibundle` files |
| `gai triggers` | Cron, webhook and file triggers |
| `gai mcp` | Manage MCP servers, or `gai mcp serve` to expose gai over stdio |
| `gai image`, `gai speak`, `gai transcribe` | Media models |
| `gai notes` | Notes on a meeting or call recording, or a video summary |
| `gai models`, `gai providers`, `gai tools`, `gai keys` | Inspect models, providers and tools; manage API keys |

Run `gai <command> --help` for details.

## Documentation

- [Agents](docs/agents.md): specs, tools, approvals, memory, knowledge, delegates and personas
- [Tools](docs/tools.md): built-ins, the code sandbox, the browser, MCP and custom tools
- [Rust SDK](docs/sdk.md): the builder, agent handles, `#[gai::tool]` and the HTTP client
- [Evaluations and feedback](docs/evals.md): eval sets, scorers, benchmarks, CI gates and ratings
- [Knowledge at scale](docs/knowledge.md): vector indexes (Qdrant, pgvector), reranking, and sources synced from folders, websites, GitHub, Confluence and Notion
- [Voice, meetings and video](docs/voice.md): talking to agents, meeting notes with speakers and action items, call analysis, video summaries
- [Databases](docs/data.md): read-only connections to Postgres, MySQL, SQLite, BigQuery and Snowflake, and the SQL tools
- [Users, teams and organizations](docs/users.md): roles, SSO (OIDC or a proxy), teams that own resources, several organizations on one server
- [Safety and governance](docs/safety.md): personal data, prompt injection, guardrails, audit log, retention
- [Bundles](docs/bundles.md): sharing agents between servers
- [Deploying](docs/deploy.md): Docker, auth and keys, data and backups
- [API reference](http://localhost:8484/docs): served by every gai server at `/docs`

## Workspace layout

| Crate | Responsibility |
|---|---|
| `gai-core` | Messages, content blocks, tool specs, usage and pricing |
| `gai-llm` | Model traits, provider dialects and presets, registry, middleware, router, media models |
| `gai-tools` | Tool trait and registry, built-in tools, MCP and A2A clients |
| `gai-sandbox` | The code sandbox (`code_exec`): container and local backends |
| `gai-browser` | The browser tools: headless Chrome, domain rules |
| `gai-store` | Store traits and the SQLite and Postgres implementations |
| `gai-memory` | Memory blocks, archival memory, knowledge ingest and hybrid search |
| `gai-skills` | Agent Skills |
| `gai-agent` | Agent specs, personas and the runtime: tool loop, approvals, durability, replay, multi-agent |
| `gai-orchestra` | YAML workflow engine |
| `gai-triggers` | Cron, webhook and file triggers |
| `gai-eval` | Eval sets, scorers, the evaluator, comparisons and the CI gate |
| `gai-macros` | `#[gai::tool]` |
| `gai` | The SDK: configuration, wiring, agent handles, bundles, HTTP client |
| `gai-server` | axum server: REST, SSE, auth, OpenAI-compatible API, MCP and A2A servers, the embedded dashboard |
| `gai-cli` | The `gai` binary |

`web/` holds the React 19 + Vite dashboard.

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd web && npm run build
```

The browser tests need Chrome or Chromium installed and skip themselves otherwise. On machines without a native Rust toolchain, `scripts/` runs everything in a podman container whose image has Chromium: `scripts/cargo.sh`, `scripts/check.sh` (fmt, clippy and tests), `scripts/serve.sh`, `scripts/sandbox-test.sh` (sandbox tests that start real containers) and `scripts/postgres-test.sh` (store tests against a throwaway Postgres).

The dashboard's components come from shadcn/ui: add one with `cd web && npx shadcn@latest add <component>` (see `web/components.json`). Claude Code users get the shadcn skill from `.claude/skills/`. After changing the API, run `npm run gen:api` in `web/` against a running server to update the TypeScript types.

Changes on their way to the next release are listed under **Unreleased** in [CHANGELOG.md](CHANGELOG.md). When you change something users will notice, add a line there in the same pull request.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion in this project, as defined in the Apache-2.0 license, is dual licensed as above, without any additional terms or conditions.
