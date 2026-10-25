//! Give an agent your own tool, written as a plain async function.
//!
//!     cargo run -p gai --example custom_tool
//!     cargo run -p gai --example custom_tool -- openai/gpt-5-mini
//!
//! With no model it uses `mock/echo`, which calls the tool directly.

use gai::agent::AgentSpec;
use gai::tools::Json;
use serde::Serialize;

#[derive(Serialize)]
struct Forecast {
    city: String,
    days: Vec<(String, i32)>,
}

/// Look up the weather forecast for a city: the high in Celsius for each of
/// the next few days.
#[gai::tool(idempotent)]
async fn forecast(
    #[arg(desc = "City name, e.g. \"Lisbon\"")] city: String,
    #[arg(desc = "How many days ahead (1-7)", default)] days: Option<u8>,
) -> Result<Json<Forecast>, String> {
    let days = days.unwrap_or(3).clamp(1, 7);
    if city.trim().is_empty() {
        return Err("give a city".into());
    }
    // A real tool would call a weather API here.
    let base = city.len() as i32 + 14;
    Ok(Json(Forecast {
        city,
        days: (1..=days)
            .map(|d| (format!("day {d}"), base + i32::from(d % 3)))
            .collect(),
    }))
}

#[tokio::main]
async fn main() -> Result<(), gai::Error> {
    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "mock/echo".into());
    let gai = gai::Gai::builder()
        .data_dir(std::env::temp_dir().join("gai-custom-tool"))
        .default_model(&model)
        .tool(forecast_tool())
        .build()?;

    let agent = AgentSpec::new("weather")
        .system("Answer weather questions with the forecast tool. Be brief.")
        .tools(["forecast"]);
    let question = if model == "mock/echo" {
        r#"call:forecast {"city": "Lisbon", "days": 2}"#
    } else {
        "Should I pack a jumper for Lisbon this weekend?"
    };
    let outcome = gai.agent_spec(agent).run(question).await?;
    println!("{}", outcome.text);
    Ok(())
}
