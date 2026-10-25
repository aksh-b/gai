//! Talk to a running `gai serve` over HTTP.
//!
//!     gai serve &                      # or scripts/serve.sh
//!     GAI_API_KEY=gai_... cargo run -p gai --example client -- "Hello!"
//!
//! `GAI_SERVER` picks the server (default http://127.0.0.1:8484).

use futures::StreamExt;
use gai::agent::RunEvent;
use gai::client::{Client, RunOptions};

#[tokio::main]
async fn main() -> Result<(), gai::client::ClientError> {
    let text = std::env::args().nth(1).unwrap_or_else(|| "Hello!".into());
    let gai = Client::from_env();
    println!("server: {}", gai.health().await?);

    // The built-in assistant, streamed as it writes.
    let mut events = gai.stream(RunOptions::default(), &text).await?;
    while let Some(e) = events.next().await {
        match e?.event {
            RunEvent::TextDelta { text } => print!("{text}"),
            RunEvent::ToolCall { name, .. } => println!("\n[{name}]"),
            _ => {}
        }
    }
    println!();
    Ok(())
}
