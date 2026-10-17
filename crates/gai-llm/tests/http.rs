//! Dialects against a fake HTTP server: SSE framing, auth headers, request
//! bodies and error mapping, end to end through the registry.

use futures::StreamExt;
use gai_llm::{
    ChatEvent, ChatRequest, LlmConfig, LlmError, ModelRegistry, ProviderConfig, RetryConfig,
    StopReason,
};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn sse(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(e, d)| format!("event: {e}\ndata: {d}\n\n"))
        .collect()
}

fn registry(name: &str, cfg: ProviderConfig) -> ModelRegistry {
    let mut c = LlmConfig {
        no_presets: true,
        ..Default::default()
    };
    c.retry = RetryConfig {
        max_retries: 1,
        initial_backoff_ms: 1,
        max_backoff_ms: 2,
    };
    c.providers.insert(name.into(), cfg);
    ModelRegistry::from_config(c).unwrap()
}

#[tokio::test]
async fn anthropic_streams_text_with_auth_headers() {
    let server = MockServer::start().await;
    let body = sse(&[
        (
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_1","model":"claude-x","usage":{"input_tokens":12,"output_tokens":1}}}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        ),
        ("ping", json!({"type":"ping"})),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":0}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-test"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut cfg = ProviderConfig::new("anthropic", &server.uri());
    cfg.api_key = Some("sk-test".to_owned().into());
    let model = registry("claude", cfg).chat("claude/claude-x").unwrap();

    let mut stream = model
        .stream(ChatRequest::user("hi").with_system("be brief"))
        .await
        .unwrap();
    let mut deltas = Vec::new();
    while let Some(e) = stream.next().await {
        if let ChatEvent::TextDelta { text, .. } = e.unwrap() {
            deltas.push(text);
        }
    }
    assert_eq!(deltas, ["Hello", " world"]);

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["model"], "claude-x");
    assert_eq!(sent["stream"], true);
    assert_eq!(sent["system"][0]["text"], "be brief");
}

#[tokio::test]
async fn openai_compatible_server_with_custom_auth_and_query() {
    let server = MockServer::start().await;
    let chunks = [
        json!({"id":"c1","model":"m","choices":[{"index":0,"delta":{"role":"assistant","content":"4"}}]}),
        json!({"id":"c1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
        json!({"id":"c1","choices":[],"usage":{"prompt_tokens":9,"completion_tokens":1}}),
    ];
    let body: String = chunks
        .iter()
        .map(|c| format!("data: {c}\n\n"))
        .collect::<String>()
        + "data: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/deploy/chat/completions"))
        .and(header("api-key", "azure-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    // An Azure-style endpoint: key in `api-key`, version in the query string.
    let mut cfg = ProviderConfig::new("openai", &format!("{}/deploy", server.uri()));
    cfg.api_key = Some("azure-key".to_owned().into());
    cfg.auth_header = Some("api-key".into());
    cfg.auth_scheme = Some(String::new());
    cfg.query.insert("api-version".into(), "2026-01-01".into());
    let model = registry("azure", cfg).chat("azure/gpt-x").unwrap();

    let r = model.complete(ChatRequest::user("2+2?")).await.unwrap();
    assert_eq!(r.text(), "4");
    assert_eq!(r.stop_reason, StopReason::EndTurn);
    assert_eq!(r.usage.input_tokens, 9);
    let req: &Request = &server.received_requests().await.unwrap()[0];
    assert_eq!(req.url.query(), Some("api-version=2026-01-01"));
}

#[tokio::test]
async fn http_errors_map_to_typed_errors_after_retries() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "0")
                .set_body_json(json!({"error":{"message":"slow down"}})),
        )
        .expect(2) // first try + one retry
        .mount(&server)
        .await;
    let model = registry("o", ProviderConfig::new("openai", &server.uri()))
        .chat("o/m")
        .unwrap();
    let err = model.complete(ChatRequest::user("x")).await.unwrap_err();
    assert!(
        matches!(err, LlmError::RateLimited { ref message, .. } if message == "slow down"),
        "{err:?}"
    );
}

#[tokio::test]
async fn missing_key_names_the_variable() {
    let cfg =
        ProviderConfig::new("openai", "http://127.0.0.1:9").key_env("GAI_TEST_DEFINITELY_UNSET");
    let err = registry("x", cfg).chat("x/m").err().unwrap();
    assert!(
        err.to_string().contains("GAI_TEST_DEFINITELY_UNSET"),
        "{err}"
    );
}

#[tokio::test]
async fn openai_embeddings() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"index": 1, "embedding": [0.0, 1.0]}, {"index": 0, "embedding": [1.0, 0.0]}],
            "usage": {"prompt_tokens": 4}
        })))
        .mount(&server)
        .await;
    let e = registry("o", ProviderConfig::new("openai", &server.uri()))
        .embedding("o/emb")
        .unwrap();
    let r = e.embed(&["a".into(), "b".into()]).await.unwrap();
    assert_eq!(r.vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
    assert_eq!(r.usage.input_tokens, 4);
}
