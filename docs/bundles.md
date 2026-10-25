# Bundles

A `.gaibundle` file carries agents, and everything they use, from one gai
server to another: to share a setup with a colleague, move from a laptop to
a server, or keep a template.

```bash
gai bundle export -a researcher -o research.gaibundle --name "Research kit"
gai bundle inspect research.gaibundle        # what's inside, what it needs
gai bundle import research.gaibundle         # on the other server
```

In the dashboard, use **Share** on the Assistants page: export the selected
assistant or everything, or import a file (you see its contents and what it
needs before anything is saved).

## What goes in

Name agents (`-a`), workflows (`-w`), personas, skills, MCP servers
(`--mcp`) or knowledge collections (`-k`), or pass `--all` for every stored
agent, workflow, persona, skill and MCP server (`--all-knowledge` adds the
collections). By default a bundle also takes what the chosen items use:

- the agents a workflow runs, and workflows it nests;
- an agent's delegates, persona, skills and knowledge collections;
- MCP servers whose tools an agent lists (`mcp__<server>__*`);
- the agents' memory blocks, with their current values.

`--no-deps` takes only what you name. Knowledge collections travel as
their documents' text and are embedded again on import, with the
collection's embedding model if this server has it and the default one
otherwise.

## Secrets

Bundles never contain API keys or tokens. In MCP server configs, a literal
header or environment value (`Authorization: Bearer sk-...`) is replaced by
a variable (`${GAI_MCP_TRACKER_AUTHORIZATION}`), and values that already
were `${VARS}` stay as they are. `bundle.toml` lists every variable the
items read and the provider keys their models need:

```toml
format = 1
name = "Research kit"
gai_version = "0.1.0"
created_at = "2026-10-24T15:41:15Z"

[contents]
agents = ["researcher", "writer"]
personas = ["concise-engineer"]
mcp_servers = ["tracker"]

[requires]
models = ["anthropic/claude-sonnet-5-5"]

[[requires.env]]
name = "ANTHROPIC_API_KEY"
used_by = "models from anthropic"

[[requires.env]]
name = "GAI_MCP_TRACKER_AUTHORIZATION"
used_by = "MCP server tracker (headers Authorization)"
```

`inspect` and `import` say which of these are not set on the receiving
server, and which models it can't reach.

## Importing

Each item is saved as a new version of the item with the same name, noted
"imported from bundle ...", so nothing is lost and you can roll back. Pass
`--skip-existing` (or turn on **Keep what's already here**) to leave
existing items alone. Memory blocks that already exist are always left
alone, since they hold what the agent has learned since.

## The file

A bundle is a zip:

```text
bundle.toml
agents/<name>.yaml
workflows/<name>.yaml
personas/<name>.yaml
skills/<name>/SKILL.md, plus the skill's files
mcp/<name>.json
memory/blocks.json
knowledge/<name>/collection.json
knowledge/<name>/00001.md ...
```

Bundles up to 200 MB (50 MB per file) are accepted. The API is
`POST /v1/bundles/export` (JSON options, returns the zip),
`/v1/bundles/inspect` and `/v1/bundles/import?skip_existing=true` (the zip
as the body); all three need an `admin` key. From Rust:
`gai::bundle::{export, inspect, import}`.
