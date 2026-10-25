//! Run an agent from Rust and stream what it does.
//!
//!     cargo run -p gai --example quickstart
//!     cargo run -p gai --example quickstart -- anthropic/claude-sonnet-5-5
//!
//! With no model it uses `mock/echo`, which needs no key and repeats the
//! input (or calls a tool when the input is `call:<tool> <json>`).

use gai::agent::{AgentSpec, RunEvent};

#[tokio::main]
async fn main() -> Result<(), gai::Error> {
    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "mock/echo".into());
    let data = std::env::temp_dir().join("gai-quickstart");
    let gai = gai::Gai::builder()
        .data_dir(&data)
        .default_model(&model)
        .build()?;

    let agent = AgentSpec::new("helper")
        .system("You are a careful assistant. Use the calculator for arithmetic.")
        .tools(["calculator"]);
    let task = if model == "mock/echo" {
        r#"call:calculator {"expression": "17% of 2340"}"#
    } else {
        "What is 17% of 2,340?"
    };

    let mut run = gai.agent_spec(agent).stream(task).await?;
    while let Some(e) = run.next_event().await {
        match e.event {
            RunEvent::ToolCall { name, input, .. } => println!("-> {name} {input}"),
            RunEvent::ToolResult { output, .. } => println!("<- {}", output.as_text()),
            _ => {}
        }
    }
    let outcome = run.wait().await;
    println!(
        "\n{}\n\n({} steps, ${:.4})",
        outcome.text, outcome.steps, outcome.cost
    );
    Ok(())
}
