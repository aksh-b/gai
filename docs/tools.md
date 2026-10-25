# Tools

An agent lists the tools it may use in `tools:` by name, or by prefix with
`*`. `gai tools` (or Settings → Tools in the dashboard) shows every tool
this server offers and its default policy.

## Built-in tools

| Tool | What it does | Default policy |
|---|---|---|
| `calculator` | Arithmetic with decimals and percentages (`17% of 2340`) | auto |
| `current_time` | The current date, time (UTC) and weekday | auto |
| `fs_read`, `fs_write`, `fs_edit`, `fs_list` | Files in the agent's workspace (paths can't leave it) | auto |
| `http_fetch` | GET a URL and return readable text. Private and loopback addresses are refused unless `[tools] allow_private_network = true` | auto |
| `shell` | Run a command in the workspace. `[hooks] deny_shell` refuses dangerous patterns | approve |

Agents also get tools from their own settings: `rag_search` (from
`knowledge`), `memory_append`, `memory_replace`, `memory_rethink`,
`remember` and `memory_search` (from `memory`), `load_skill` (from
`skills`), `ask_<agent>` (from `delegates`) and `image_gen`, `speak` and
`transcribe` (when media models are set).

`[hooks] redact_secrets` (on by default) masks API keys, tokens and
passwords in tool output before the model sees it.

## The code sandbox: `code_exec`

`code_exec {language, code, timeout_secs}` runs Python, JavaScript (node)
or bash. With the default `container` backend each call starts a fresh
Docker or Podman container with no network, 512 MB of memory, one CPU and
a 60 s limit; the session's workspace is copied in and changed files are
copied back. Files the code writes are saved with the run, and images
(charts saved as PNG) are shown in chat and to the model.

```toml
[sandbox]
backend = "container"          # "local" runs on the host (approval needed); "off"
# engine = "unix:///run/podman/podman.sock"   # default: DOCKER_HOST, then the usual sockets
memory_mb = 512
timeout_secs = 60
network = false
[sandbox.images]
python = "docker.io/library/python:3.13-slim"
```

The default Python image has no pandas or matplotlib.
`scripts/sandbox-image.sh` builds one that does
(`localhost/gai-sandbox-python`); point `images.python` at it. The **Data
analyst** template uses the sandbox.

## The browser

With Chrome or Chromium installed (or `--build-arg BROWSER=1` in Docker),
agents get a real headless browser:

| Tool | What it does |
|---|---|
| `browser_navigate` | Open a URL and get an outline of the page: headings, text, and every link, button and field with a number like `[3]` |
| `browser_read` | The outline again (`mode: outline`), or the readable text of the main content (`mode: text`) |
| `browser_click` | Click a link, button, checkbox or tab by `ref` (a number) or CSS `selector` |
| `browser_type` | Type into a field, or choose an option in a dropdown |
| `browser_submit` | Submit a form. Needs approval by default |
| `browser_extract` | Every element matching a CSS selector, as JSON with the attributes you ask for |
| `browser_screenshot` | A PNG of the page (or the whole page), shown to the model and the user |

Each chat gets its own browser context (cookies and logins stay in that
chat) that closes after 5 minutes unused. Every request a page makes,
including redirects and images, is checked against the rules:

```toml
[browser]
# executable = "/usr/bin/chromium"    # default: chromium or google-chrome on PATH
allow_domains = []                    # empty: any public site
deny_domains = ["facebook.com"]       # a domain covers its subdomains
allow_private_network = false         # localhost, 10.x, 192.168.x, cloud metadata
timeout_secs = 30
idle_secs = 300
```

The buttons that submit a form can't be pressed with `browser_click`; the
agent has to use `browser_submit`, so a person sees what is about to be
sent. The **Web browser** template uses these tools.

## MCP servers

Tools from [Model Context Protocol](https://modelcontextprotocol.io)
servers appear as `mcp__<server>__<tool>`. Add servers in `gai.toml`, with
`gai mcp add`, or in the dashboard (Connect), which also handles OAuth
sign-in.

```toml
[mcp_servers.fs]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/data"]

[mcp_servers.tracker]
url = "https://mcp.example.com/mcp"
headers = { Authorization = "Bearer ${TRACKER_TOKEN}" }   # ${VAR} from the environment
policy = "approve"                                        # or per tool in tool_policy
```

gai is an MCP server too: `/mcp` over streamable HTTP, or `gai mcp serve`
over stdio, exposing agents and workflows as tools and knowledge
collections as resources.

## Your own tools in Rust

Write a documented async function and add `#[gai::tool]`. The doc comment
becomes the description, the parameters the input schema, and the return
value the output.

```rust
/// Look up an order by its number and return its status.
#[gai::tool(idempotent)]
async fn order_status(
    #[arg(desc = "The order number, e.g. A-1042")] number: String,
    #[arg(default)] include_items: bool,
) -> Result<String, String> {
    if !number.starts_with("A-") {
        return Err(format!("{number} is not an order number"));
    }
    Ok(format!("{number}: shipped{}", if include_items { " (2 items)" } else { "" }))
}

let gai = gai::Gai::builder().tool(order_status_tool()).build()?;
```

Options: `#[gai::tool(name = "..", policy = "approve", idempotent)]`.
`Option<T>` parameters are optional, and a `ToolCtx` or `&ToolCtx`
parameter receives the call's context (run, session, workspace, a
cancellation token) instead of an input. Return text, numbers,
`serde_json::Value`, `gai::tools::Json(anything_serializable)`, a
`ToolOutput`, or a `Result` of those; an `Err` is reported to the model as
a failed call. For full control, implement `gai::tools::Tool` yourself.
