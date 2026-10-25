# The Rust SDK

The `gai` crate is the whole platform as a library: the same runtime,
store and tools that `gai serve` uses, inside your program. Add it from the
repository until it is published:

```toml
[dependencies]
gai = { git = "https://github.com/aksh-b/gai" }
tokio = { version = "1", features = ["full"] }
```

The default features include the code sandbox's container backend and the
browser tools. `default-features = false` leaves both out.

## Building a gai

```rust
let gai = gai::Gai::builder()
    .config("gai.toml")                 // or .with_config(Config { .. }); default: the usual places
    .data_dir("./data")                 // database, workspaces, files
    .default_model("anthropic/claude-sonnet-5-5")
    .tool(my_tool())                    // extra tools, e.g. from #[gai::tool]
    .build()?;
```

`Gai::load(None)` does the same with only the config file. Provider keys
come from the environment (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, ...) or
the config. The store opens on first use, so a program that only calls
models never creates a database.

## Running agents

```rust
use gai::agent::AgentSpec;

// A stored agent or an agent file, by reference: "researcher",
// "researcher@v3", "researcher@prod" or "agents/researcher.yaml".
let out = gai.agent("researcher").run("Brief me on WebGPU").await?;
println!("{} ({} steps, ${:.4})", out.text, out.steps, out.cost);

// An agent written in code.
let spec = AgentSpec::new("helper").system("Be brief.").tools(["calculator"]);
let out = gai.agent_spec(spec).run("What is 17% of 2,340?").await?;

// Options: continue a conversation, pick a persona or model, skip approvals.
let next = gai
    .agent("researcher")
    .session(&out.session_id)
    .persona("concise-engineer")
    .model("openai/gpt-5-mini")
    .run("And Safari?")
    .await?;
```

`run_with(blocks)` takes any input (text, images, audio). An agent whose
`output_schema` is set returns parsed JSON in `out.output`.

### Streaming

```rust
use gai::agent::RunEvent;

let mut run = gai.agent("researcher").stream("Brief me on WebGPU").await?;
while let Some(e) = run.next_event().await {
    match e.event {
        RunEvent::TextDelta { text } => print!("{text}"),
        RunEvent::ToolCall { name, input, .. } => eprintln!("\n-> {name} {input}"),
        RunEvent::ApprovalRequired { id, name, .. } => eprintln!("\n{name} needs approval ({id})"),
        _ => {}
    }
}
let outcome = run.wait().await;
```

A tool with the `approve` policy pauses the run. Resolve it with
`gai.runtime().await?.resolve_approval(&run_id, &call_id, decision)`, or
call `.auto_approve()` on the agent handle when nobody is there to ask.

### Going lower

`gai.runtime().await?` is the agent runtime: save and load agents
(`save_agent`, `load_agent`), personas, knowledge (`knowledge()`), skills
(`skills()`), MCP servers, approvals, and `run`/`start` with a full
`RunRequest`. `gai.workflows()` runs workflows and `gai.triggers()`
manages schedules and webhooks. Each crate is re-exported: `gai::agent`,
`gai::llm`, `gai::tools`, `gai::store`, `gai::memory`, `gai::orchestra`,
`gai::skills`, `gai::triggers`, `gai::sandbox` and `gai::browser`.

To call a model without an agent:

```rust
let model = gai.chat_model(Some("anthropic/claude-sonnet-5-5"))?;
```

## Tools as functions

```rust
use gai::tools::Json;

/// Convert an amount between currencies at today's rate.
#[gai::tool(idempotent)]
async fn convert(
    #[arg(desc = "Amount to convert")] amount: f64,
    #[arg(desc = "ISO code, e.g. EUR")] from: String,
    to: String,
) -> Result<Json<serde_json::Value>, String> {
    let rate = lookup_rate(&from, &to).await.map_err(|e| e.to_string())?;
    Ok(Json(serde_json::json!({"amount": amount * rate, "currency": to})))
}

let gai = gai::Gai::builder().tool(convert_tool()).build()?;
```

The macro keeps the function and adds `<name>_tool()`. See
[Tools](tools.md#your-own-tools-in-rust) for the options.

## The HTTP client

`gai::client::Client` talks to a running `gai serve`:

```rust
use futures::StreamExt;
use gai::client::{Client, RunOptions};

let gai = Client::new("https://gai.example.com").with_key("gai_...");
// or Client::from_env(): GAI_SERVER and GAI_API_KEY

let out = gai.run("researcher", "Brief me on WebGPU").await?;

let mut events = gai
    .stream(RunOptions::agent("researcher").session(&out.session_id), "And Safari?")
    .await?;
while let Some(e) = events.next().await {
    println!("{:?}", e?.event);
}

gai.approvals().await?;                        // calls waiting for a person
gai.decide(&run_id, &call_id, &decision).await?;
gai.cancel(&run_id).await?;
let runs: serde_json::Value = gai.get("/v1/runs?limit=5").await?;  // any endpoint
```

Errors from the server come back as `ClientError::Api { status, message }`.

## Examples

```bash
cargo run -p gai --example quickstart                 # offline, with mock/echo
cargo run -p gai --example quickstart -- anthropic/claude-sonnet-5-5
cargo run -p gai --example custom_tool
GAI_API_KEY=... cargo run -p gai --example client -- "Hello!"
```
