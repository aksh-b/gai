# Changelog

All notable changes to gai are written down here, so you can see what is
coming before it is released. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and gai uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until 1.0,
minor versions may contain breaking changes; they are always marked
**Breaking** below.

No version has been released yet. Everything up to the first entries below
(the model layer, agents and tools, MCP, memory and knowledge, workflows,
versioning and replay, triggers, A2A, media models and the router) is the
initial development and will ship as 0.1.0.

## [Unreleased]

### Added

- **Notifications.** Rules under **Settings › Notifications** say who
  hears when something needs an OK, a run fails or finishes, or a
  dashboard alert fires: by email, Slack (an incoming webhook), a webhook,
  or as browser notifications (web push, no third-party service). Rules
  can be narrowed to some assistants and sent as a test. New `[notify]`
  options `dashboard_url`, `push_subject` and `push_key_env`. See
  [docs/notifications.md](docs/notifications.md).
- **Suggestions.** **Activity › Suggestions** collects things worth
  doing, each with a button that does it: improve an assistant that keeps
  failing, add the Database analyst for a database nothing queries, put a
  connected GitHub or Gmail to work, schedule a question you keep asking.
  Agents add their own with the new `suggest` tool; the **Proactive
  watcher** template checks your data on a schedule and suggests what to
  do. See [docs/suggestions.md](docs/suggestions.md).
- **Specialist assistant templates:** a Code assistant (writes, runs and
  reviews code; reviews GitHub pull requests when GitHub is connected), a
  Translator that keeps a glossary of your terms, and an Email and message
  drafter that reads threads and saves drafts in Gmail or Outlook when
  they are connected. A **GitHub** connection template sets up GitHub's
  MCP server in a click under **Settings › Tools & connections**.
- **Optional tools.** An agent's tool ending in `?` (`mcp__github__*?`)
  is used when it exists, and the agent runs without it otherwise.
- `tool_policy` keys can be prefixes ending in `*`
  (`mcp__gmail__send*: approve`).
- Dashboard alerts and report deliveries can go to **Slack**
  (`slack` or `slack_env` next to `email` and `webhook`).

- **Marketplace.** Publish assistants, workflows and prompts from a server
  as listings (bundles with a title, tags and versions), rate them, and
  install them in a click, after seeing what they need. Other gai servers
  can be added as registries to browse and install from. See
  [docs/marketplace.md](docs/marketplace.md).
- **Prompt templates.** Instructions kept on their own under
  **Assistants › Prompts**, versioned and labelled like agents, with
  variables and defaults. Agents use one with `prompt: name@label`, before
  their own `system`; bundles carry them. See [docs/prompts.md](docs/prompts.md).
- **Prompt optimizer.** **Improve** on a test set rewrites an assistant's
  instructions (or its prompt template) from the cases that failed, keeps
  only rewrites that score higher, and saves the best as a new version
  when you say so. `POST /v1/evals/{name}/optimize`.
- **Mock models** `mock/obey` and `mock/rewriter`, to try the optimizer
  without a real model.
- **Apps.** Publish an assistant or workflow as a small app under
  **Automations › Apps**: a form (text, numbers, yes/no, choices) with
  its own link, made without YAML. Share it with a link carrying the
  app's own key, which runs that app and nothing else, or make it public.
  Every app is also an endpoint, `POST /v1/apps/{name}/run`, that checks
  its input against the fields, can answer in a JSON Schema you give, and
  has its own OpenAPI document. See [docs/apps.md](docs/apps.md).
- **Workflow designer.** Workflows open on a canvas: add steps, drag
  between them to make one wait for another, delete steps and lines, and
  arrange them. Positions are saved under the workflow's new `layout`, so
  the canvas, the step list and the YAML stay in step.
- **Voice conversations.** **Talk** in the chat holds a spoken conversation
  with any agent: each pause ends a turn, the answer is spoken sentence by
  sentence as it is written, and talking over it interrupts. Agents with
  `media.realtime` (`openai/gpt-realtime`) go through the provider's
  speech-to-speech session, their tools running in gai. Over the API:
  the `/v1/voice` WebSocket.
- **Meeting notes, call analysis and video summaries.** Drop a recording
  into **Knowledge › Recordings** (or `POST /v1/recordings`, `gai notes`):
  meetings get speakers, a summary, decisions, action items with owners
  and dates, and template sections (standup, one-to-one, sales call,
  interview, retro, lecture); calls get talk time, language, sentiment
  and key moments; videos get frames described by a vision model and a
  summary in chapters. Agents get `meeting_notes`, `analyze_audio` and
  `summarize_video`, and a Meeting notes template. Long recordings and
  video use ffmpeg, now in the Docker image (`--build-arg MEDIA=0` leaves
  it out).
- **Speakers and timestamps in transcripts.** Transcriptions carry timed
  segments with speakers where the model gives them (Whisper,
  `gpt-4o-transcribe-diarize`, ElevenLabs Scribe): `gai transcribe
  --speakers`, `"speakers": true` on `/v1/audio/transcriptions`, and the
  `transcribe` tool's `speakers`. `[media]` gains `vision` and `realtime`.
- **Dashboards, alerts and reports.** Dashboards of SQL panels (line, bar,
  area, pie, number, table) that refresh on a schedule, built by agents
  with `save_dashboard` or under **Data › Dashboards**; panel alerts email
  or call a webhook when a KPI crosses a limit. Triggers take `deliver`
  to send their run's answer and files: scheduled reports. `[notify]`
  configures SMTP (or a folder of `.eml` files).
- **Data-analysis skill.** Drivers of a change, forecasts, anomalies and
  data-quality checks in Python for `code_exec`, installed with the
  Database analyst template.
- **Databases.** Read-only connections to Postgres, MySQL, SQLite, BigQuery
  and Snowflake (`gai data`, the **Data** page, `/v1/connections`), with
  their tables read for agents. Tools `sql_schema`, `sql_query` (one
  `SELECT`, row limits, dry runs, results saved as CSV) and `sql_execute`
  (writes, with approval, on connections that allow them), and a
  **Database analyst** template. See [docs/data.md](docs/data.md).
- **Knowledge graphs.** A model reads a collection for entities and the
  relations between them (`gai collections graph`, the **Graph** tab,
  `/v1/collections/{name}/graph`); graph search (`mode: graph`, and
  `rag_search` when a collection has a graph) also finds passages through
  those connections and lists them. **Table** tab for extraction.
- **OCR.** Images and scanned PDFs are read when they are added, with
  Tesseract (in the Docker image; `--build-arg OCR=0` leaves it out) or a
  vision model (`[knowledge.ocr]`).
- **Whole-document tools.** `summarize_document`, `compare_documents`,
  `extract_fields` (a collection's documents into a table and a CSV) and
  `write_document` (Markdown, Word or PDF files), plus a **Contract
  reviewer** template that uses them. `POST /v1/collections/{name}/extract`
  returns the table as JSON or CSV. `mock/fields` fills fields offline from
  `Name: value` lines.
- **Organizations.** One server can host several organizations that never
  see each other's data: each gets its own database (a SQLite file, or a
  Postgres schema) and files, and is confined (no `shell`, no private
  addresses, HTTP MCP servers only, files only in its own folder, tokens
  from its own secrets). `gai orgs`, `/v1/orgs`, monthly budgets and rate
  limits per organization, and branding (name, logo, colour) in the
  dashboard. See [docs/users.md](docs/users.md).
- **Users, roles and teams.** People sign in with OIDC
  (`[server.sso.oidc]`) or through a proxy header, and are viewers,
  members, builders, admins or owners of each organization. Teams own
  assistants, workflows, knowledge and the rest, so only their builders
  change them; team keys are limited the same way. `gai members`,
  `gai teams`, **Settings › People & teams**.
- **Search chats, analytics and @mentions.** Find words across every
  conversation (**Activity › Search chats**, `GET /v1/sessions/search`),
  see how each assistant version does (runs, success, steps, latency, cost,
  ratings: **Activity › By assistant**, `GET /v1/analytics/agents`), hand a
  chat turn to another assistant with `@name`, and see who changed an
  assistant last (`GET /v1/changes`).
- **Knowledge only some may find.** A source's `acl` (`team:`, `user:`,
  `role:`) limits who finds what it brings in, in searches, document lists
  and agents' `rag_search`.
- **Build scope.** Keys get a `build` scope between `run` and `admin`:
  changing agents, personas, workflows, skills, triggers, sources and evals
  now needs `build` (admin keys still can). **Breaking** for scripts that
  check exact scope names in `/v1/me`.
- **Safety settings** (`[safety]` in gai.toml, `safety:` per agent, and a
  Safety section on each assistant). Personal data (emails, phones, card
  numbers, IBANs, SSNs, IPs, Aadhaar and PAN) can be noted, masked before
  the model sees it, or refused. Tool results that try to give the model
  instructions (prompt injection) are flagged, stripped or withheld, and
  the model is told that tool results are data. `deny_topics`,
  `deny_output` and `max_tool_calls` add guardrails. Warnings show in the
  run's events. Hooks get `HookCtx::warn` and the run's safety settings.
- **Audit log** of every change made through the API and dashboard: who,
  what, when, the status and safe details (never secrets). `gai audit`
  (also `--csv`), `GET /v1/audit`, and Settings › Audit & data.
- **Retention and forgetting people.** `retention_days` deletes old
  conversations hourly (`gai retention sweep` to do it now); `gai
  forget-user <id>` and `DELETE /v1/users/{id}` delete everything kept about
  a person. `gai bundle export --redact-pii` masks personal data in bundles.
- **Evaluations.** An eval set (YAML) lists cases, each an input and how
  to judge the answer: `contains`, `regex`, `equals`, `json` field checks,
  `tool_called`, step/cost/latency limits, an LLM `judge` with a rubric,
  `grounded` (claims backed by what the agent retrieved) and `cites` (names
  a source it used). `gai eval run <set> -a agent@v3` scores every case,
  stores the results, and exits with status 1 when the pass rate is below
  the set's threshold or the score drops against `--baseline`, so it can
  gate CI. Several `-m` flags benchmark models side by side (score, cost,
  latency). Also `gai eval list|show|save|runs|report|compare`, and
  `/v1/evals`, `/v1/eval-runs`. `mock/judge` grades offline for trying it
  out; see `examples/evals/calc.yaml`.
- **Feedback on replies.** Thumbs up and down under every chat reply; a
  thumbs down asks what went wrong. `POST /v1/runs/{id}/feedback`,
  `/v1/feedback` and `/v1/feedback/summary` (per agent version). **Add
  thumbs-down replies** turns disliked replies into eval cases.
- **Regenerate** under the last chat reply runs the turn again as a fork,
  keeping the old reply in its own history.
- **Tests** in the dashboard (Assistants › Tests): write eval sets, run them
  on an assistant and up to three models, read per-case results, and
  compare two evaluations.
- **Code sandbox.** Agents can run Python, JavaScript or bash with the new
  `code_exec` tool. Each call runs in a fresh Docker or Podman container with
  no network access, capped memory and CPU, and a time limit. The container
  is thrown away afterwards. The chat's files are available to the code, and
  files it writes (charts, CSVs) are saved with the run and shown in chat.
  Configure it under `[sandbox]` in `gai.toml`. Set `backend = "local"` to
  run code without isolation (development only; each call asks for
  approval), or `"off"` to turn the tool off.
- A **Data analyst** template that answers questions about your data by
  running code and drawing charts.
- **Browser tools.** Agents can use a real (headless) Chrome or Chromium:
  `browser_navigate` opens a page and returns an outline in which every
  link, button and field has a number, `browser_click`, `browser_type` and
  `browser_submit` act on those numbers, `browser_read` gives the outline or
  the readable text, `browser_extract` collects elements as JSON, and
  `browser_screenshot` shows the page to the model and the user. Each chat
  gets its own browser context, closed after 5 minutes unused. Every request
  a page makes is checked against `allow_domains` and `deny_domains`, and
  private addresses are refused unless `allow_private_network` is set.
  Submitting a form asks for approval by default. The tools appear when
  Chrome or Chromium is installed; configure them under `[browser]` in
  `gai.toml`. A **Web browser** template uses them.
- **Personas**: reusable voices (name, avatar, greeting, tone, formality,
  length, language, do/don't rules and example replies) that any assistant
  can speak in. Give an assistant a default persona with `persona:` (and
  `persona_placement: prepend | append`, or put `{{ persona }}` in its
  instructions), pick one for a single chat, or set a default for every chat
  that has none. Personas are versioned like assistants, with diffs, labels
  and rollback. The dashboard has a Personas tab with four templates,
  "Draft it for me", a live "Try it" panel, and the persona's avatar,
  greeting and starters in chat. API: `/v1/personas` and a `persona` field
  on `POST /v1/runs`; gai.toml: `[persona] default`, `placement`.
- **Rust SDK.** `Gai::builder()` sets the config file, data folder,
  default model, store and extra tools in code. `gai.agent("name")` and
  `gai.agent_spec(spec)` give an agent handle with `.run(text)`,
  `.stream(text)` (events as they happen), `.session(id)`, `.persona(..)`
  and `.model(..)`. `#[gai::tool]` turns a documented async function into
  a tool: the doc comment is its description, the parameters its input
  schema, and the return value (text, numbers, JSON or a `Result`) its
  output. `gai::client::Client` talks to a running server over HTTP: run,
  stream, follow, cancel and approve. **Breaking:** the old
  `Gai::agent(reference)` lookup is now `Gai::find_agent`.
- **Bundles.** A `.gaibundle` file carries assistants and everything they
  use (personas, skills, delegate assistants, workflows, knowledge
  collections with their documents, MCP servers and starting memory) to
  another gai server. API keys and tokens are never written: secret values
  become `${VARS}`, and the bundle lists the variables and models it needs.
  `gai bundle export|inspect|import`, `/v1/bundles/{export,inspect,import}`,
  and **Share** on the Assistants page.
- **Docker image.** `docker build -t gai .` makes a single-binary image
  (Debian slim, dashboard included, data in `/data`); add
  `--build-arg BROWSER=1` for the browser tools.
- **Sources.** Keep a knowledge collection in step with a folder, a
  website (URLs, a sitemap, same-site links), a GitHub repository, a
  Confluence space or Notion pages. Re-syncs only add, replace and remove
  what changed; `every_minutes` syncs on a schedule while `gai serve` runs.
  `gai sources save|sync|list|show|delete` and `/v1/sources`. See
  [docs/knowledge.md](docs/knowledge.md).
- **Vector indexes.** `[knowledge.vector_index] backend = "qdrant"` (with
  `url`) or `"pgvector"` searches vectors there instead of scanning the
  store, which stays the source of truth: ingesting, replacing and deleting
  documents keep the index in sync, and `gai collections reindex <name>`
  (`POST /v1/collections/{name}/reindex`) fills it. If the index is down,
  search falls back to the store.
- **Reranking.** `[knowledge.rerank]` reorders the best hybrid results with
  a rerank API (`backend = "http"`: Cohere, Jina, Voyage or TEI) or a chat
  model (`backend = "llm"`). Hits carry a `rerank_score`.
- **Postgres.** Set `database = "postgres://user:pass@host/db"` in gai.toml
  (or `GAI_DATABASE`) to keep everything in Postgres instead of SQLite:
  conversations, runs, knowledge (full-text search with tsvector), memory,
  evals, the audit log and the rest. Both backends pass the same
  conformance suite (`scripts/postgres-test.sh`). **Breaking** for anyone
  who tried the earlier groundwork: the Postgres schema was redone; drop
  that test database.
- **SDK examples:** `cargo run -p gai --example quickstart`, `custom_tool`
  and `client`.
- **The dashboard is built into the `gai` binary.** `gai serve` shows it
  without a `web/dist` folder next to it; build `web/` before building gai
  to include it (a binary built without it shows how). `--ui <dir>` still
  serves a build from disk, and `--no-ui` serves only the API.
- **A code editor for YAML and SKILL.md** in the dashboard (CodeMirror),
  with syntax colours, folding, search, and inline errors. Assistant,
  workflow, persona and trigger YAML is checked against the server's
  schemas as you type (unknown keys, wrong types, missing fields), and
  top-level keys are suggested.
- **Workflow graphs can be panned and zoomed** (xyflow), with live step
  colours while a workflow runs.
- **First-run setup** in the dashboard: connect a cloud provider by pasting a
  key, download a local model from the Ollama library, or try the demo. On a
  local server the dashboard signs itself in, with no API key to paste.
- **Settings from the dashboard**: the default model and provider API keys
  can be changed without editing `gai.toml` or restarting.
- **Templates** for assistants and workflows, installable in one click, and
  "Draft it for me": describe an assistant in one sentence.
- A **step-by-step workflow editor**, alongside the YAML editor.
- Chat: drag in or paste files, approve tool calls inline, and save the
  current setup as an assistant.

### Changed

- The repository no longer ships a `gai.toml`: it was one developer's
  machine setup (local Ollama models and a locally built sandbox image),
  which a fresh clone picked up and then failed on. `gai.example.toml` is
  the template; copy it to `gai.toml`, which git now ignores.
- The dashboard is built entirely from [shadcn/ui](https://ui.shadcn.com)
  components (on Radix and Tailwind CSS) instead of the flame library and
  hand-written styles, so it builds from a plain clone with `npm install`.
  It looks a little different: neutral colours, Lucide icons, a collapsible
  sidebar that also works on phones, cards and lists throughout.
- Chat uses shadcn's chat components: replies follow as they stream with a
  jump-to-latest button, thinking and tool calls show as one activity trace
  you can expand, and attachments show as file chips.
- Deleting, revoking, rolling back and signing out ask for confirmation in a
  dialog instead of the browser's pop-up.
- Spending shows a bar chart per model, and a run's steps show on a
  timeline. Rows in History and Models open with a single click.
- The dashboard is reorganised into six sections (Chat, Assistants,
  Automations, Knowledge, Activity, Settings) and uses plain wording.
  Technical details (IDs, YAML, raw events) are behind **Developer details**
  in Settings → Preferences. Old page links still work.
- Ollama models get a 12k-token context window by default.
- `scripts/serve.sh` keeps the Ollama address from `base_url` under
  `[providers.ollama]` in `gai.toml`, so gai can use Ollama on another
  machine. Before, the script always replaced it with an Ollama it found
  itself. `GAI_PROVIDERS__OLLAMA__BASE_URL` still overrides both.

### Fixed

- Running an agent with no model set now says how to choose one (`-m`,
  `default_model`, or a provider key) instead of only that there is none.
- The calculator did integer division (`17 / 100 * 2340` gave 0), and now
  understands `X% of Y`.
- A very long tool result, such as a whole web page, could overflow a small
  model's context and fail the run. Tool results are now capped at a quarter
  of the model's context window.
- Knowledge search finds paraphrased questions more reliably: embedding
  models that expect task prefixes get them, and common words no longer
  swamp keyword matches.
- Ollama refused tools whose input has no parameters (such as
  `current_time`).

### Security

- The container sandbox needs the Docker/Podman socket, and access to that
  socket is equivalent to root on the engine's host. Only the gai process
  uses it; code written by agents runs in a locked-down container and never
  sees it.
