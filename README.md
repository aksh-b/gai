# gai

A self-hostable agentic AI platform in Rust. You define agents in YAML. They can use any model provider, call tools and MCP servers, remember things, search your documents and delegate to each other. You run them from a CLI, an HTTP API (including an OpenAI-compatible one) or a web dashboard.

Website: https://aksh-b.github.io/gai

> Status: early and moving fast (v0.1). Phases 1–8 and 10 of a 12-phase plan are built; see [STATUS.md](STATUS.md) for details and known gaps.

## Features

- **Any model.** Dialects for Anthropic, OpenAI-compatible APIs and ElevenLabs, plus presets for OpenAI, Gemini, Groq, OpenRouter, DeepSeek, Mistral, Together, Fireworks, xAI, Ollama, LM Studio and vLLM. Models are addressed as `provider/model`. Retry, fallback and cost metering are built in, and `router/<name>` picks the cheapest capable model per request.
- **Agents.** A tool loop with parallel tool calls, approval policies, budgets, context compaction, and durable runs that resume after a restart.
- **Tools and MCP.** Built-ins (calculator, time, files, HTTP fetch, shell) and an MCP client (stdio and streamable HTTP, with OAuth sign-in).
- **Memory and RAG.** Self-editing memory blocks, archival memory, and knowledge collections with hybrid search (vector + FTS5, fused with RRF) over text, Markdown, HTML and PDF.
- **Agent Skills.** `SKILL.md` folders or uploaded skills, loaded on demand.
- **Multi-agent.** Delegation through `ask_<agent>` tools, and YAML DAG workflows with parallel nodes, `map` fan-out, `if`, retries, human approval steps and checkpoints.
- **Versioning and replay.** Every save of an agent or workflow is a version (diff, label, roll back). Any run can be forked from a step with a different model, prompt or tools, then compared side by side.
- **Media.** Speech-to-text, text-to-speech and image generation as tools and endpoints.
- **Triggers.** Cron schedules, HMAC-signed webhooks and file watchers.
- **Interop.** gai runs as an MCP server (`/mcp` or stdio) and an A2A server and client.
- **Server.** REST + SSE API, API keys with scopes, per-key budgets and rate limits, an OpenAI-compatible `/openai/v1`, and OpenAPI 3.1 with interactive docs at `/docs`.
- **Dashboard.** React UI for the playground, runs, approvals, agents, workflows, knowledge, memory, skills, triggers, media, models, tools, usage and keys.

Storage is SQLite behind swappable store traits.

## Quick start

Requirements: Rust 1.91+, and Node 20+ for the dashboard.

```bash
git clone https://github.com/aksh-b/gai.git
cd gai
cargo build -p gai-cli
```

Everything works offline with the built-in `mock/echo` model:

```bash
cargo run -p gai-cli -- run -a examples/agents/calc.yaml "call:calculator {\"expression\": \"6*7\"}"
cargo run -p gai-cli -- workflow run examples/workflows/hello.yaml -i names='["ada", "alan"]'
```

To use a real model, set the provider's key and pick a model:

```bash
export ANTHROPIC_API_KEY=...
cargo run -p gai-cli -- chat -m anthropic/claude-sonnet-5-5
```

### Server and dashboard

The dashboard imports components from the flame UI library, which it expects in a sibling folder (`../flame`).

```bash
cd web && npm install && npm run build && cd ..
cargo run -p gai-cli -- serve
```

Open http://localhost:8484 for the dashboard and http://localhost:8484/docs for the API reference. Auth is open while no API key exists and the server is local. Create the first key with:

```bash
cargo run -p gai-cli -- keys create --name me --scope admin
```

For dashboard development, run `npm run dev` in `web/` (port 5190, proxying `/v1` to :8484).

## Configuration

gai reads `gai.toml` from the working directory, and any value can be overridden with `GAI_*` environment variables (for example `GAI_DEFAULT_MODEL=ollama/gemma3:1b`). Every option is documented in [crates/gai/src/config.rs](crates/gai/src/config.rs).

```toml
# A local model from Ollama
[[providers.ollama.models]]
id = "gemma3:1b"
capabilities = { tools = false, context_window = 32768 }

# router/auto: easy chat goes local, harder work goes to a bigger model
[routers.auto]
models = [
  { model = "ollama/gemma3:1b", level = "easy" },
  { model = "anthropic/claude-sonnet-5-5", level = "hard" },
]
```

## Defining an agent

```yaml
model: anthropic/claude-sonnet-5-5
description: Researches a topic and writes a short brief.
system: You are {{ agent }}. Today is {{ date }}.
tools: [http_fetch, rag_search]
knowledge: [handbook]
delegates: [writer]
limits:
  max_steps: 20
```

See [examples/agents](examples/agents) and [examples/workflows](examples/workflows) for more.

## CLI

| Command | What it does |
|---|---|
| `gai serve` | HTTP API and dashboard |
| `gai run` / `gai chat` / `gai ask` | One-shot run, terminal chat UI, quick question |
| `gai agents`, `gai workflow` | Manage, version, diff and roll back agents and workflows |
| `gai runs`, `gai sessions`, `gai replay` | Inspect and resume runs; fork a run from any step |
| `gai ingest`, `gai collections`, `gai memory`, `gai skills` | Knowledge, memory and skills |
| `gai triggers` | Cron, webhook and file triggers |
| `gai mcp` | Manage MCP servers, or `gai mcp serve` to expose gai over stdio |
| `gai image`, `gai speak`, `gai transcribe` | Media models |
| `gai models`, `gai providers`, `gai tools`, `gai keys` | Inspect models, providers and tools; manage API keys |

Run `gai <command> --help` for details.

## Workspace layout

| Crate | Responsibility |
|---|---|
| `gai-core` | Messages, content blocks, tool specs, usage and pricing |
| `gai-llm` | Model traits, provider dialects and presets, registry, middleware, router, media models |
| `gai-tools` | Tool trait and registry, built-in tools, MCP and A2A clients |
| `gai-store` | Store traits and the SQLite implementation |
| `gai-memory` | Memory blocks, archival memory, knowledge ingest and hybrid search |
| `gai-skills` | Agent Skills |
| `gai-agent` | Agent specs and the runtime: tool loop, approvals, durability, replay, multi-agent |
| `gai-orchestra` | YAML workflow engine |
| `gai-triggers` | Cron, webhook and file triggers |
| `gai` | Facade: configuration and wiring |
| `gai-server` | axum server: REST, SSE, auth, OpenAI-compatible API, MCP and A2A servers |
| `gai-cli` | The `gai` binary |

`web/` holds the React 19 + Vite dashboard.

## Development

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

On machines without a native Rust toolchain, `scripts/` runs everything in a podman container: `scripts/cargo.sh`, `scripts/check.sh` (fmt, clippy and tests), `scripts/serve.sh` and `scripts/ollama.sh`.

## License

MIT OR Apache-2.0.
