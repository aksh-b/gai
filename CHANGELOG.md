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
