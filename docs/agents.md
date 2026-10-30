# Agents

An agent is a spec: a model, instructions, tools and limits, plus optional
memory, knowledge, skills, delegates and a persona. Specs are YAML (or TOML
or JSON) files, or rows in the database that you create with the CLI, the
API or the dashboard. Every save of a stored agent is a new version.

```yaml
# researcher.yaml
description: Researches a question on the web and reports concise findings.
model: anthropic/claude-sonnet-5-5
system: |
  You are {{ agent }}. Today is {{ date }}. Use http_fetch to read sources,
  keep what is relevant, and cite each finding's URL.
tools: [http_fetch, browser_*, calculator]
tool_policy: { shell: approve }
reasoning: { effort: medium }
limits: { max_steps: 30, max_cost_usd: 1.0 }
```

```bash
gai agents save researcher.yaml          # stored as researcher v1
gai run -a researcher "What changed in Rust 1.91?"
gai run -a researcher.yaml "..."         # or run the file without saving
gai agents diff researcher 1 2           # also: versions, label, rollback
```

## The spec

| Key | What it does |
|---|---|
| `name` | The agent's name (taken from the file name or the save call when left out). |
| `description` | One line. Other agents see it when this one is a delegate, and the dashboard shows it. |
| `model` | `provider/model`, an alias from `[aliases]`, or `router/<name>`. Defaults to `default_model`. |
| `system` | The instructions, a [minijinja](https://docs.rs/minijinja) template. Variables: `agent`, `model`, `date`, `datetime`, `workspace`, `persona`, and the run's `vars`. |
| `tools` | Tool names, or prefixes ending in `*` (`fs_*`, `browser_*`, `mcp__tracker__*`). An entry ending in `?` is optional (`mcp__github__*?`): the agent uses those tools when they exist and runs without them when they don't, instead of failing. |
| `language` | The language it answers in: a code (`fr`, `pt-BR`) or a name (`French`). The instructions can place it with `{{ language }}`; otherwise a line asking for replies in it goes at the end. A run can pick another (`"language"` on `POST /v1/runs`), and without either the server's default applies (**Settings › Preferences**, or `language` in gai.toml). |
| `tool_policy` | Per tool: `auto`, `approve` (a person approves each call) or `deny`. Overrides the tool's default. Keys are tool names or prefixes ending in `*` (`mcp__gmail__send*: approve`); the longest match wins. |
| `reasoning` | `effort: off\|low\|medium\|high\|max`, optional `budget_tokens`, and `expose: true` to keep readable reasoning. Mapped to what each provider offers. |
| `max_tokens`, `temperature` | Passed to the model. |
| `output_schema` | A JSON Schema the final answer must follow; the run's `output` is then parsed JSON. |
| `workspace` | The folder file and shell tools work in. Defaults to a folder per session under the data dir. |
| `limits` | `max_steps` (50), `max_tokens`, `max_cost_usd`, `timeout_secs`, `max_parallel_tools` (8). Hitting one ends the run as `budget_exceeded`. |
| `context` | When to clear old tool results (`clear_tool_results_at`, 0.6 of the window) and summarize old turns (`compact_at`, 0.8), what to keep, the summary model, and prompt caching. |
| `memory` | Memory blocks and archival memory (below). |
| `knowledge` | Collections the agent searches with `rag_search`. |
| `skills` | Skills it can load: names or prefixes ending in `*`. |
| `delegates` | Agents it can hand work to, each as an `ask_<name>` tool. |
| `persona`, `persona_placement` | The voice it speaks in (below). |
| `media` | Models for `image_gen`, `speak`, `transcribe` and the recording tools (`vision` for video frames), and `realtime` for live voice, over `[media]`. See [voice](voice.md). |
| `metadata` | Free-form data for your own integrations. |

The dashboard checks agent YAML against this schema as you type. The
schema is also in the API's OpenAPI document as `AgentSpec`.

## Tools and approvals

Tools come from the built-ins, the code sandbox, the browser, MCP servers,
your own Rust tools, and per-agent tools that memory, knowledge and skills
add. [Tools](tools.md) lists them. A tool whose policy is `approve` pauses
the run until someone approves or denies the call: in the dashboard
(Approvals, or inline in chat), with `gai run`'s prompt, or through
`POST /v1/runs/{id}/approvals/{call_id}`. A paused run holds no memory and
survives restarts.

## Memory

```yaml
memory:
  blocks:
    - label: notes
      description: Things worth remembering about this project.
      limit: 2000
    - label: human
      scope: user            # the user's profile, shared by every agent
    - label: policy
      scope: shared:support  # shared by every agent naming it
      read_only: true
      value: Refunds over $100 need a manager.
  archival: true             # remember / memory_search
```

Blocks are always in the agent's context, and it edits them with
`memory_append`, `memory_replace` and `memory_rethink`. Every edit is
versioned (`gai memory history`). Archival memory is a searchable store the
agent writes to with `remember` and reads with `memory_search`.

## Knowledge

```bash
gai ingest ./handbook -c handbook      # text, Markdown, HTML and PDF
gai collections search handbook "parental leave"
```

An agent with `knowledge: [handbook]` gets `rag_search`, which runs hybrid
search: vector similarity plus keyword search, fused. The embedding model
is `[knowledge] embedding_model` (for example `ollama/nomic-embed-text`).

## Delegates and workflows

```yaml
delegates:
  - writer                        # a stored agent: becomes ask_writer
  - agent: reviewer@prod          # a labelled version
    description: Check anything before it goes to a customer.
  - a2a: https://agents.example.com/a2a/translator
    name: translator              # a remote agent over A2A
```

For fixed pipelines, use a workflow instead: a YAML graph of agent, tool,
template, value and human-approval nodes that run in parallel when they
can. See [examples/workflows](../examples/workflows) and
`gai workflow --help`.

## Personas

A persona is a reusable voice: tone, formality, length, language, do and
don't rules, example replies, and a display name, avatar, greeting and
starter prompts for chat. It is stored and versioned like an agent.

```yaml
# concise-engineer.yaml
display_name: Concise engineer
avatar: "🛠️"
tone: direct and precise
verbosity: terse
dos: [Lead with the answer, Show code instead of describing it]
donts: [Pad answers with pleasantries]
```

An agent picks one with `persona: concise-engineer` (or `name@v2`,
`name@prod`). A run can override it (`persona` on `POST /v1/runs`, or
`"none"`), and `[persona] default` in `gai.toml` (or the Personas page)
sets one for runs that have none. The persona goes before the instructions
(`persona_placement: prepend`), after them (`append`), or wherever the
system prompt says `{{ persona }}`.

## Versions, labels and replay

Saving a stored agent makes a new version unless nothing changed. Point a
label at a version (`gai agents label researcher prod 3`) and refer to it
as `researcher@prod`, or to any version as `researcher@v3`. Rolling back
saves the old spec as a new version.

Every run records each model request, so it can be forked:
`gai replay <run> --from 3 -m openai/gpt-5-mini` replays steps 1-2 from the
recording without calling the model and runs the rest live. The dashboard
shows forks side by side.
