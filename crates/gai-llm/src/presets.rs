//! Built-in provider presets. Each is ordinary [`ProviderConfig`]; a user
//! entry with the same name is merged over it.
//!
//! Almost every hosted and local model server speaks the OpenAI Chat
//! Completions format, so most presets are the `openai` dialect with a base
//! URL and a key variable.

use std::collections::BTreeMap;

use gai_core::Pricing;

use crate::{Capabilities, MediaPricing, ModelConfig, ModelKind, ProviderConfig};

pub fn presets() -> BTreeMap<String, ProviderConfig> {
    let openai_compat = |base: &str, key: &str| {
        ProviderConfig::new("openai", base)
            .key_env(key)
            .option("max_tokens_field", "max_tokens")
    };
    let local =
        |base: &str| ProviderConfig::new("openai", base).option("max_tokens_field", "max_tokens");

    let mut p = BTreeMap::new();
    p.insert("anthropic".into(), anthropic());
    let mut openai = ProviderConfig::new("openai", "https://api.openai.com/v1")
        .key_env("OPENAI_API_KEY")
        .option("max_tokens_field", "max_completion_tokens");
    openai.models = openai_media();
    p.insert("openai".into(), openai);
    let mut gemini = openai_compat(
        "https://generativelanguage.googleapis.com/v1beta/openai",
        "GEMINI_API_KEY",
    );
    gemini.models.push(ModelConfig::media(
        "imagen-4.0-generate-001",
        ModelKind::Image,
        per_image(0.04),
    ));
    p.insert("gemini".into(), gemini);
    p.insert("elevenlabs".into(), elevenlabs());
    // whisper.cpp's server (`whisper-server`); its model is the one it was
    // started with, so any model name works: `whispercpp/default`.
    p.insert(
        "whispercpp".into(),
        ProviderConfig::new("openai", "http://localhost:8080")
            .option("transcription_path", "/inference"),
    );
    p.insert(
        "groq".into(),
        openai_compat("https://api.groq.com/openai/v1", "GROQ_API_KEY"),
    );
    p.insert(
        "openrouter".into(),
        openai_compat("https://openrouter.ai/api/v1", "OPENROUTER_API_KEY"),
    );
    p.insert(
        "deepseek".into(),
        openai_compat("https://api.deepseek.com/v1", "DEEPSEEK_API_KEY"),
    );
    p.insert(
        "mistral".into(),
        openai_compat("https://api.mistral.ai/v1", "MISTRAL_API_KEY"),
    );
    p.insert(
        "together".into(),
        openai_compat("https://api.together.xyz/v1", "TOGETHER_API_KEY"),
    );
    p.insert(
        "fireworks".into(),
        openai_compat("https://api.fireworks.ai/inference/v1", "FIREWORKS_API_KEY"),
    );
    p.insert(
        "xai".into(),
        openai_compat("https://api.x.ai/v1", "XAI_API_KEY"),
    );
    p.insert(
        "ollama".into(),
        local("http://localhost:11434/v1").option("think_tags", true),
    );
    p.insert(
        "lmstudio".into(),
        local("http://localhost:1234/v1").option("think_tags", true),
    );
    p.insert(
        "vllm".into(),
        local("http://localhost:8000/v1").option("think_tags", true),
    );
    p.insert(
        "mock".into(),
        ProviderConfig {
            dialect: "mock".into(),
            ..Default::default()
        },
    );
    p
}

fn per_image(usd: f64) -> MediaPricing {
    MediaPricing {
        per_image: Some(usd),
        ..Default::default()
    }
}

fn per_minute(usd: f64) -> MediaPricing {
    MediaPricing {
        per_minute: Some(usd),
        ..Default::default()
    }
}

fn per_million_chars(usd: f64) -> MediaPricing {
    MediaPricing {
        per_million_chars: Some(usd),
        ..Default::default()
    }
}

/// OpenAI speech and image models with approximate list prices (USD).
/// gpt-image-1 is priced by tokens; the per-image figure is a medium-quality
/// 1024x1024 image.
fn openai_media() -> Vec<ModelConfig> {
    vec![
        ModelConfig::media("whisper-1", ModelKind::Transcription, per_minute(0.006)),
        ModelConfig::media(
            "gpt-4o-transcribe",
            ModelKind::Transcription,
            per_minute(0.006),
        ),
        ModelConfig::media(
            "gpt-4o-mini-transcribe",
            ModelKind::Transcription,
            per_minute(0.003),
        ),
        ModelConfig::media("tts-1", ModelKind::Speech, per_million_chars(15.0)),
        ModelConfig::media("tts-1-hd", ModelKind::Speech, per_million_chars(30.0)),
        ModelConfig::media(
            "gpt-4o-mini-tts",
            ModelKind::Speech,
            per_million_chars(12.0),
        ),
        ModelConfig::media("gpt-image-1", ModelKind::Image, per_image(0.042)),
        ModelConfig::media("dall-e-3", ModelKind::Image, per_image(0.04)),
    ]
}

/// ElevenLabs voices and transcription, with approximate prices.
fn elevenlabs() -> ProviderConfig {
    let mut cfg = ProviderConfig::new("elevenlabs", "https://api.elevenlabs.io")
        .key_env("ELEVENLABS_API_KEY");
    cfg.models = vec![
        ModelConfig::media(
            "eleven_flash_v2_5",
            ModelKind::Speech,
            per_million_chars(75.0),
        ),
        ModelConfig::media(
            "eleven_multilingual_v2",
            ModelKind::Speech,
            per_million_chars(150.0),
        ),
        ModelConfig::media("scribe_v1", ModelKind::Transcription, per_minute(0.0067)),
    ];
    cfg
}

/// Claude models with their context windows, output caps and list prices
/// (USD per million tokens, first-party API, as of 2026-10).
fn anthropic() -> ProviderConfig {
    let caps = |context: u32, output: u32| Capabilities {
        tools: true,
        vision: true,
        reasoning: true,
        json_schema: true,
        prompt_caching: true,
        audio: false,
        context_window: Some(context),
        max_output_tokens: Some(output),
        dimensions: None,
    };
    const M: u32 = 1_000_000;
    const K128: u32 = 128_000;
    let models = [
        (
            "claude-fable-5-1",
            caps(M, K128),
            Pricing::new(10.0, 50.0).with_cache_read(0.25),
        ),
        ("claude-fable-5", caps(M, K128), Pricing::new(10.0, 50.0)),
        (
            "claude-opus-5-5",
            caps(M, K128),
            Pricing::new(4.0, 20.0).with_cache_read(0.20),
        ),
        ("claude-opus-5", caps(M, K128), Pricing::new(5.0, 25.0)),
        ("claude-opus-4-8", caps(M, K128), Pricing::new(5.0, 25.0)),
        ("claude-opus-4-7", caps(M, K128), Pricing::new(5.0, 25.0)),
        ("claude-opus-4-6", caps(M, K128), Pricing::new(5.0, 25.0)),
        (
            "claude-sonnet-5-5",
            caps(M, K128),
            Pricing::new(2.0, 10.0).with_cache_read(0.20),
        ),
        ("claude-sonnet-5", caps(M, K128), Pricing::new(2.0, 10.0)),
        ("claude-sonnet-4-6", caps(M, K128), Pricing::new(3.0, 15.0)),
        ("claude-haiku-5-5", caps(M, K128), Pricing::new(0.10, 0.50)),
        (
            "claude-haiku-4-5",
            caps(200_000, 64_000),
            Pricing::new(1.0, 5.0),
        ),
    ];
    let mut cfg =
        ProviderConfig::new("anthropic", "https://api.anthropic.com").key_env("ANTHROPIC_API_KEY");
    cfg.default_capabilities = Some(caps(200_000, 32_000));
    cfg.models = models
        .into_iter()
        .map(|(id, c, p)| ModelConfig::chat(id, c, Some(p)))
        .collect();
    cfg
}
