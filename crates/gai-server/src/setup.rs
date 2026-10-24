//! First-run setup for the dashboard: signing this browser in without the
//! CLI, API keys for providers, the default model, pulling Ollama models,
//! and starter templates.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures::StreamExt;
use gai::agent::{AgentSpec, PersonaSpec};
use gai::llm::{Capabilities, ChatRequest, KeyStatus, ModelConfig, ModelKind};
use gai::orchestra::WorkflowSpec;
use gai::store::{Scope, keys};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/setup/status", get(status))
        .route("/v1/setup/claim", post(claim))
        .route("/v1/settings", get(get_settings).put(put_settings))
        .route(
            "/v1/providers/{name}/key",
            put(put_provider_key).delete(delete_provider_key),
        )
        .route("/v1/providers/{name}/test", post(test_provider))
        .route("/v1/providers/ollama/library", get(ollama_library))
        .route("/v1/providers/ollama/pull", post(ollama_pull))
        .route("/v1/providers/ollama/pulls", get(ollama_pulls))
        .route("/v1/templates", get(list_templates))
        .route("/v1/templates/{id}/install", post(install_template))
}

/// Cloud providers the dashboard offers to connect, with where to get a key.
const CLOUD: &[(&str, &str, &str, &str)] = &[
    (
        "anthropic",
        "Anthropic (Claude)",
        "Strong at writing, coding and using tools.",
        "https://console.anthropic.com/settings/keys",
    ),
    (
        "openai",
        "OpenAI (GPT)",
        "Popular all-rounder; also does images and voice.",
        "https://platform.openai.com/api-keys",
    ),
    (
        "gemini",
        "Google Gemini",
        "Fast, with a generous free tier.",
        "https://aistudio.google.com/apikey",
    ),
    (
        "openrouter",
        "OpenRouter",
        "One key for hundreds of models from many companies.",
        "https://openrouter.ai/keys",
    ),
];

/// Local models worth suggesting, smallest first.
const OLLAMA_LIBRARY: &[(&str, &str, &str, bool)] = &[
    (
        "gemma3:1b",
        "Small and fast. Good for quick chat; cannot use tools.",
        "0.8 GB",
        false,
    ),
    (
        "qwen3:4b-instruct",
        "Can use tools (files, web, calculator). A good first choice.",
        "2.5 GB",
        true,
    ),
    (
        "llama3.2:3b",
        "Balanced chat model from Meta; can use tools.",
        "2.0 GB",
        true,
    ),
    (
        "qwen3:8b",
        "Smarter but slower; needs 8 GB of free memory.",
        "5.2 GB",
        true,
    ),
    (
        "nomic-embed-text",
        "Makes document search understand meaning (Knowledge).",
        "0.3 GB",
        false,
    ),
];

// ---- status and claiming ----

#[derive(Serialize, JsonSchema)]
pub struct SetupStatus {
    /// Requests without a key are refused, and this one had no valid key.
    pub needs_key: bool,
    /// This browser can get a key from `POST /v1/setup/claim`.
    pub can_claim: bool,
    /// A real chat model (not the demo model) is ready to use.
    pub has_chat_model: bool,
    pub default_model: Option<String>,
    /// `settings`, `config` or `detected`.
    pub default_model_source: Option<String>,
    pub ollama: OllamaStatus,
    pub providers: Vec<CloudProvider>,
}

#[derive(Serialize, JsonSchema)]
pub struct OllamaStatus {
    pub reachable: bool,
    pub base_url: Option<String>,
    /// Installed models, by Ollama name.
    pub models: Vec<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct CloudProvider {
    pub name: String,
    pub title: String,
    pub description: String,
    pub key_url: String,
    /// The provider has a key (from the dashboard, config or environment).
    pub connected: bool,
    /// Last four characters of a key saved from the dashboard.
    pub saved_key: Option<String>,
    /// A model to try first with this provider.
    pub suggested_model: Option<String>,
}

async fn status(State(s): State<AppState>, headers: HeaderMap) -> ApiResult<Json<SetupStatus>> {
    let store = s.rt.store();
    let needs_key = !s.auth.open(store).await && !has_valid_key(&s, &headers).await;
    let models = s.gai.models();
    let ollama = ollama_status(&s).await;
    if ollama.reachable {
        register_ollama_models(&s, &ollama.models).await;
    }
    let has_chat_model = models.catalog().iter().any(|e| {
        let i = &e.info;
        i.kind == ModelKind::Chat
            && !matches!(i.provider.as_str(), "mock" | "router")
            && match i.provider.as_str() {
                "ollama" => ollama.models.iter().any(|m| m == &i.model),
                p => match models.key_status(p) {
                    KeyStatus::Present => true,
                    KeyStatus::NotNeeded => !LOCAL_PRESETS.contains(&p),
                    KeyStatus::Missing(_) => false,
                },
            }
    });
    let has_chat_model = has_chat_model
        || CLOUD
            .iter()
            .any(|(name, ..)| models.key_status(name) == KeyStatus::Present);
    let (default_model, source) = match s.gai.default_model_with_source() {
        Some((m, src)) => (Some(m), Some(src.to_owned())),
        None => (None, None),
    };
    let providers = CLOUD
        .iter()
        .filter(|(name, ..)| models.providers().any(|(n, _)| n == *name))
        .map(|(name, title, description, key_url)| CloudProvider {
            name: (*name).into(),
            title: (*title).into(),
            description: (*description).into(),
            key_url: (*key_url).into(),
            connected: models.key_status(name) == KeyStatus::Present,
            saved_key: models.stored_key_hint(name),
            suggested_model: suggested_model(&s, name),
        })
        .collect();
    Ok(Json(SetupStatus {
        needs_key,
        // Browsers send no Origin on a same-origin GET; `claim` (a POST)
        // checks it for real.
        can_claim: needs_key && can_claim(&s, &headers, false),
        has_chat_model,
        default_model,
        default_model_source: source,
        ollama,
        providers,
    }))
}

/// Local servers whose preset catalog says nothing about what is running.
const LOCAL_PRESETS: &[&str] = &["ollama", "lmstudio", "vllm", "llamacpp", "whispercpp"];

fn suggested_model(s: &AppState, provider: &str) -> Option<String> {
    let models = s.gai.models();
    let (_, cfg) = models.providers().find(|(n, _)| *n == provider)?;
    let chat: Vec<_> = cfg
        .models
        .iter()
        .filter(|m| m.kind == ModelKind::Chat)
        .collect();
    // Presets list the flagship first; a mid-size model is the better default.
    let pick = chat
        .iter()
        .find(|m| m.id.contains("sonnet") || m.id.contains("flash") || m.id.contains("mini"))
        .or(chat.first())?;
    Some(format!("{provider}/{}", pick.id))
}

/// Model names to look for, best first, when a provider's catalog lists no
/// chat model and its live model list has to be searched instead.
const PREFERRED: &[(&str, &[&str])] = &[
    (
        "openai",
        &["gpt-5-mini", "gpt-5", "gpt-4.1-mini", "gpt-4o-mini"],
    ),
    ("gemini", &["gemini-2.5-flash", "flash", "gemini"]),
    (
        "openrouter",
        &[
            "anthropic/claude-sonnet",
            "openai/gpt-5-mini",
            "google/gemini",
        ],
    ),
];

/// A chat model to use with `provider`: from its catalog, or else picked
/// from the models it serves and added to the catalog, so pickers show it.
async fn ensure_chat_model(s: &AppState, provider: &str) -> Option<String> {
    if let Some(m) = suggested_model(s, provider) {
        return Some(m);
    }
    let served = s.gai.models().discover(provider).await.ok()?;
    let ids: Vec<String> = served
        .into_iter()
        .map(|m| m.id.strip_prefix("models/").unwrap_or(&m.id).to_owned())
        .filter(|id| {
            ![
                "audio",
                "tts",
                "image",
                "embed",
                "realtime",
                "transcribe",
                "search",
                "moderation",
            ]
            .iter()
            .any(|x| id.contains(x))
        })
        .collect();
    let wanted = PREFERRED
        .iter()
        .find(|(p, _)| *p == provider)
        .map(|(_, w)| *w)
        .unwrap_or(&[]);
    let pick = wanted
        .iter()
        .find_map(|w| ids.iter().find(|id| id.contains(w)))
        .or(ids.first())?
        .clone();
    let model = ModelConfig::chat(&pick, Capabilities::default(), None);
    if let Err(e) = s.gai.add_model(provider, model).await {
        tracing::warn!(provider, error = %e, "could not add a model to the catalog");
    }
    Some(format!("{provider}/{pick}"))
}

async fn has_valid_key(s: &AppState, headers: &HeaderMap) -> bool {
    let Some(token) = crate::auth::header_token(headers) else {
        return false;
    };
    matches!(
        s.rt.store().find_key(&keys::hash(&token)).await,
        Ok(Some(_))
    )
}

/// A browser on this machine, on the dashboard's own page, may get a key
/// without the CLI: the server must be local-only (`--local` or a loopback
/// bind), and the request must come from a page served by this server under
/// a loopback name. The `Origin` check stops other websites from minting
/// keys; the loopback-name check stops DNS rebinding.
fn can_claim(s: &AppState, headers: &HeaderMap, require_origin: bool) -> bool {
    if !s.auth.local {
        return false;
    }
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let origin = match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        Some(o) => o,
        None if !require_origin => return is_loopback_host(host),
        None => return false,
    };
    let Some(authority) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    authority.eq_ignore_ascii_case(host) && is_loopback_host(host)
}

fn is_loopback_host(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map_or(host, |(n, _)| n),
    }
    .to_ascii_lowercase();
    name == "localhost" || name.ends_with(".localhost") || name == "127.0.0.1" || name == "::1"
}

#[derive(Serialize, JsonSchema)]
pub struct Claimed {
    pub key: gai::store::ApiKey,
    /// Shown once; the dashboard keeps it in the browser.
    pub secret: String,
}

async fn claim(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<(StatusCode, Json<Claimed>)> {
    if !can_claim(&s, &headers, true) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "this server only hands out keys to its own dashboard on this computer; create one with `gai keys create`",
        ));
    }
    let k = keys::generate();
    let name = format!(
        "dashboard on this computer ({})",
        chrono::Utc::now().format("%Y-%m-%d")
    );
    let key =
        s.rt.store()
            .create_key(&name, &k.prefix, &k.hash, &[Scope::Admin], None)
            .await?;
    Ok((
        StatusCode::CREATED,
        Json(Claimed {
            key,
            secret: k.secret,
        }),
    ))
}

// ---- settings and provider keys ----

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct Settings {
    /// The model used when an agent names none. `null` goes back to the
    /// one in gai.toml.
    pub default_model: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct SettingsView {
    pub default_model: Option<String>,
    pub default_model_source: Option<String>,
}

fn settings_view(s: &AppState) -> SettingsView {
    match s.gai.default_model_with_source() {
        Some((m, src)) => SettingsView {
            default_model: Some(m),
            default_model_source: Some(src.into()),
        },
        None => SettingsView {
            default_model: None,
            default_model_source: None,
        },
    }
}

async fn get_settings(State(s): State<AppState>) -> Json<SettingsView> {
    Json(settings_view(&s))
}

async fn put_settings(
    State(s): State<AppState>,
    Json(b): Json<Settings>,
) -> ApiResult<Json<SettingsView>> {
    let model = b
        .default_model
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty());
    s.gai.set_default_model(model).await?;
    Ok(Json(settings_view(&s)))
}

#[derive(Deserialize, JsonSchema)]
pub struct ProviderKey {
    pub key: String,
}

#[derive(Serialize, JsonSchema)]
pub struct ProviderKeyView {
    pub provider: String,
    pub connected: bool,
    /// Last four characters of the saved key; the key itself is never sent back.
    pub saved_key: Option<String>,
}

fn key_view(s: &AppState, provider: &str) -> ProviderKeyView {
    let models = s.gai.models();
    ProviderKeyView {
        provider: provider.to_owned(),
        connected: models.key_status(provider) == KeyStatus::Present,
        saved_key: models.stored_key_hint(provider),
    }
}

async fn put_provider_key(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Json(b): Json<ProviderKey>,
) -> ApiResult<Json<ProviderKeyView>> {
    if b.key.trim().is_empty() {
        return Err(ApiError::bad_request("the key is empty"));
    }
    s.gai
        .set_provider_key(&name, Some(b.key))
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    // Best effort: a provider whose catalog lists no chat model gets one
    // from its live list, so there is something to pick right away.
    let _ = ensure_chat_model(&s, &name).await;
    Ok(Json(key_view(&s, &name)))
}

async fn delete_provider_key(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<ProviderKeyView>> {
    s.gai
        .set_provider_key(&name, None)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(key_view(&s, &name)))
}

#[derive(Default, Deserialize, JsonSchema)]
pub struct TestRequest {
    /// The model to try; defaults to a suggested model of the provider.
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct TestResult {
    pub ok: bool,
    pub model: String,
    /// What went wrong, in plain words.
    pub message: String,
}

async fn test_provider(
    State(s): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<TestRequest>>,
) -> ApiResult<Json<TestResult>> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let model = match req.model {
        Some(m) => Some(m),
        None => ensure_chat_model(&s, &name).await,
    };
    let model = match model {
        Some(m) => m,
        None => {
            return Err(ApiError::bad_request(format!(
                "{name} lists no chat model to try"
            )));
        }
    };
    let result = async {
        let chat = s.gai.models().chat(&model)?;
        let mut r = ChatRequest::user("Reply with the single word OK.");
        r.max_tokens = Some(16);
        tokio::time::timeout(Duration::from_secs(60), chat.complete(r))
            .await
            .map_err(|_| gai::llm::LlmError::Config("no answer within a minute".into()))?
    }
    .await;
    Ok(Json(match result {
        Ok(_) => TestResult {
            ok: true,
            model,
            message: "Connected.".into(),
        },
        Err(e) => TestResult {
            ok: false,
            model,
            message: plain_error(&e.to_string()),
        },
    }))
}

/// Provider errors, reworded for someone who has never seen an HTTP status.
fn plain_error(e: &str) -> String {
    let l = e.to_lowercase();
    if l.contains("401")
        || l.contains("unauthorized")
        || l.contains("invalid x-api-key")
        || l.contains("invalid api key")
        || l.contains("incorrect api key")
    {
        "The key was not accepted. Check that you copied all of it.".into()
    } else if l.contains("402")
        || l.contains("credit")
        || l.contains("billing")
        || l.contains("quota")
    {
        format!("The key works, but the account has no credit left or needs billing set up. ({e})")
    } else if l.contains("429") || l.contains("rate") {
        format!("The provider is busy or limiting requests; try again in a minute. ({e})")
    } else if l.contains("connect")
        || l.contains("dns")
        || l.contains("timed out")
        || l.contains("within a minute")
    {
        format!("Could not reach the provider. Check the internet connection. ({e})")
    } else {
        e.to_owned()
    }
}

// ---- Ollama ----

/// Ollama's native API root (the provider's base URL without `/v1`).
fn ollama_root(s: &AppState) -> Option<String> {
    let models = s.gai.models();
    let (_, cfg) = models.providers().find(|(n, _)| *n == "ollama")?;
    let base = cfg.base_url.as_deref()?.trim_end_matches('/');
    Some(base.strip_suffix("/v1").unwrap_or(base).to_owned())
}

async fn ollama_status(s: &AppState) -> OllamaStatus {
    let Some(root) = ollama_root(s) else {
        return OllamaStatus {
            reachable: false,
            base_url: None,
            models: Vec::new(),
        };
    };
    let tags =
        s.rt.http()
            .get(format!("{root}/api/tags"))
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .ok()
            .filter(|r| r.status().is_success());
    let models = match tags {
        Some(r) => r.json::<Value>().await.ok().map(|v| {
            v["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m["name"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        }),
        None => None,
    };
    OllamaStatus {
        reachable: models.is_some(),
        base_url: Some(root),
        models: models.unwrap_or_default(),
    }
}

/// Add installed Ollama models that gai.toml does not list to the catalog,
/// with what Ollama says they can do, so pickers show them and runs use them
/// correctly (a model without tool support must not be sent tools).
async fn register_ollama_models(s: &AppState, installed: &[String]) {
    let models = s.gai.models();
    let Some((_, cfg)) = models.providers().find(|(n, _)| *n == "ollama") else {
        return;
    };
    let known: Vec<String> = cfg.models.iter().map(|m| m.id.clone()).collect();
    let context = cfg
        .models
        .iter()
        .filter_map(|m| m.capabilities.as_ref()?.context_window)
        .max()
        .unwrap_or(4096);
    let catalog: Vec<String> = models
        .catalog()
        .into_iter()
        .filter(|e| e.info.provider == "ollama")
        .map(|e| e.info.model)
        .collect();
    for name in installed {
        if known.contains(name) || catalog.contains(name) {
            continue;
        }
        if let Some(m) = describe_ollama_model(s, name, context).await
            && let Err(e) = s.gai.add_model("ollama", m).await
        {
            tracing::warn!(model = %name, error = %e, "could not add an Ollama model");
        }
    }
}

async fn describe_ollama_model(s: &AppState, name: &str, context: u32) -> Option<ModelConfig> {
    let root = ollama_root(s)?;
    let show: Value =
        s.rt.http()
            .post(format!("{root}/api/show"))
            .json(&json!({ "model": name }))
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
    let caps: Vec<&str> = show["capabilities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if caps.contains(&"embedding") {
        return Some(ModelConfig {
            id: name.to_owned(),
            kind: ModelKind::Embedding,
            ..Default::default()
        });
    }
    let capabilities = Capabilities {
        tools: caps.contains(&"tools"),
        vision: caps.contains(&"vision"),
        reasoning: caps.contains(&"thinking"),
        context_window: Some(context),
        ..Default::default()
    };
    Some(ModelConfig::chat(name, capabilities, None))
}

#[derive(Serialize, JsonSchema)]
pub struct LibraryModel {
    pub name: String,
    pub description: String,
    pub size: String,
    pub tools: bool,
    pub installed: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct OllamaLibrary {
    pub status: OllamaStatus,
    pub models: Vec<LibraryModel>,
}

async fn ollama_library(State(s): State<AppState>) -> Json<OllamaLibrary> {
    let status = ollama_status(&s).await;
    let models = OLLAMA_LIBRARY
        .iter()
        .map(|(name, description, size, tools)| LibraryModel {
            name: (*name).into(),
            description: (*description).into(),
            size: (*size).into(),
            tools: *tools,
            installed: status
                .models
                .iter()
                .any(|m| m == name || m.strip_suffix(":latest") == Some(name)),
        })
        .collect();
    Json(OllamaLibrary { status, models })
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct PullProgress {
    pub model: String,
    /// `pulling`, `done` or `failed`.
    pub state: String,
    /// Ollama's own words for the current step.
    pub status: String,
    pub completed: u64,
    pub total: u64,
    pub error: Option<String>,
}

fn pulls() -> &'static Mutex<BTreeMap<String, PullProgress>> {
    static PULLS: OnceLock<Mutex<BTreeMap<String, PullProgress>>> = OnceLock::new();
    PULLS.get_or_init(Default::default)
}

fn set_pull(p: PullProgress) {
    pulls()
        .lock()
        .expect("pulls lock")
        .insert(p.model.clone(), p);
}

#[derive(Deserialize, JsonSchema)]
pub struct PullRequest {
    pub model: String,
}

/// Start downloading a model into Ollama. Progress is at
/// `GET /v1/providers/ollama/pulls`.
async fn ollama_pull(
    State(s): State<AppState>,
    Json(b): Json<PullRequest>,
) -> ApiResult<(StatusCode, Json<PullProgress>)> {
    let model = b.model.trim().to_owned();
    if model.is_empty() || model.contains(char::is_whitespace) {
        return Err(ApiError::bad_request("name a model, e.g. gemma3:1b"));
    }
    let root = ollama_root(&s).ok_or_else(|| ApiError::bad_request("Ollama is not configured"))?;
    if let Some(p) = pulls().lock().expect("pulls lock").get(&model)
        && p.state == "pulling"
    {
        return Ok((StatusCode::ACCEPTED, Json(p.clone())));
    }
    let start = PullProgress {
        model: model.clone(),
        state: "pulling".into(),
        status: "starting".into(),
        completed: 0,
        total: 0,
        error: None,
    };
    set_pull(start.clone());
    tokio::spawn(async move {
        let mut p = start;
        match pull(&s, &root, &mut p).await {
            Ok(()) => {
                let context = s
                    .gai
                    .models()
                    .providers()
                    .find(|(n, _)| *n == "ollama")
                    .and_then(|(_, c)| {
                        c.models
                            .iter()
                            .filter_map(|m| m.capabilities.as_ref()?.context_window)
                            .max()
                    })
                    .unwrap_or(4096);
                let listed = s
                    .gai
                    .models()
                    .providers()
                    .any(|(n, c)| n == "ollama" && c.model(&p.model).is_some());
                if !listed && let Some(m) = describe_ollama_model(&s, &p.model, context).await {
                    let _ = s.gai.add_model("ollama", m).await;
                }
                p.state = "done".into();
                p.status = "ready".into();
            }
            Err(e) => {
                p.state = "failed".into();
                p.error = Some(e);
            }
        }
        set_pull(p);
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(pulls().lock().expect("pulls lock")[&model].clone()),
    ))
}

async fn pull(s: &AppState, root: &str, p: &mut PullProgress) -> Result<(), String> {
    let resp =
        s.rt.http()
            .post(format!("{root}/api/pull"))
            .json(&json!({ "model": p.model, "stream": true }))
            .send()
            .await
            .map_err(|e| format!("could not reach Ollama: {e}"))?;
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Ollama refused: {text}"));
    }
    let mut body = resp.bytes_stream();
    let mut buf = Vec::new();
    while let Some(chunk) = body.next().await {
        buf.extend_from_slice(&chunk.map_err(|e| e.to_string())?);
        while let Some(i) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=i).collect();
            let Ok(v) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            if let Some(e) = v["error"].as_str() {
                return Err(e.to_owned());
            }
            if let Some(st) = v["status"].as_str() {
                p.status = st.to_owned();
            }
            if let (Some(c), Some(t)) = (v["completed"].as_u64(), v["total"].as_u64()) {
                p.completed = c;
                p.total = t;
            }
            set_pull(p.clone());
        }
    }
    Ok(())
}

async fn ollama_pulls() -> Json<Vec<PullProgress>> {
    Json(
        pulls()
            .lock()
            .expect("pulls lock")
            .values()
            .cloned()
            .collect(),
    )
}

// ---- templates ----

struct Template {
    id: &'static str,
    kind: &'static str,
    title: &'static str,
    /// Stored agents this one uses, installed with it when missing.
    needs: &'static [&'static str],
    yaml: &'static str,
}

const TEMPLATES: &[Template] = &[
    Template {
        id: "researcher",
        kind: "agent",
        title: "Researcher",
        needs: &[],
        yaml: include_str!("../../../examples/agents/researcher.yaml"),
    },
    Template {
        id: "writer",
        kind: "agent",
        title: "Writer",
        needs: &[],
        yaml: include_str!("../../../examples/agents/writer.yaml"),
    },
    Template {
        id: "docs-helper",
        kind: "agent",
        title: "Answers from your documents",
        needs: &[],
        yaml: include_str!("../../../examples/agents/docs-helper.yaml"),
    },
    Template {
        id: "data-analyst",
        kind: "agent",
        title: "Data analyst (runs code)",
        needs: &[],
        yaml: include_str!("../../../examples/agents/data-analyst.yaml"),
    },
    Template {
        id: "web-browser",
        kind: "agent",
        title: "Web browser (clicks and fills forms)",
        needs: &[],
        yaml: include_str!("../../../examples/agents/web-browser.yaml"),
    },
    Template {
        id: "companion",
        kind: "agent",
        title: "Remembers you",
        needs: &[],
        yaml: include_str!("../../../examples/agents/companion.yaml"),
    },
    Template {
        id: "supervisor",
        kind: "agent",
        title: "Team lead (researcher + writer)",
        needs: &["researcher", "writer"],
        yaml: include_str!("../../../examples/agents/supervisor.yaml"),
    },
    Template {
        id: "research-brief",
        kind: "workflow",
        title: "Research brief",
        needs: &["researcher", "writer"],
        yaml: include_str!("../../../examples/workflows/research-brief.yaml"),
    },
    Template {
        id: "hello",
        kind: "workflow",
        title: "Hello (a tour, runs without a model)",
        needs: &[],
        yaml: include_str!("../../../examples/workflows/hello.yaml"),
    },
    Template {
        id: "concise-engineer",
        kind: "persona",
        title: "Concise engineer",
        needs: &[],
        yaml: include_str!("../../../examples/personas/concise-engineer.yaml"),
    },
    Template {
        id: "friendly-tutor",
        kind: "persona",
        title: "Friendly tutor",
        needs: &[],
        yaml: include_str!("../../../examples/personas/friendly-tutor.yaml"),
    },
    Template {
        id: "formal-analyst",
        kind: "persona",
        title: "Formal analyst",
        needs: &[],
        yaml: include_str!("../../../examples/personas/formal-analyst.yaml"),
    },
    Template {
        id: "playful-companion",
        kind: "persona",
        title: "Playful companion",
        needs: &[],
        yaml: include_str!("../../../examples/personas/playful-companion.yaml"),
    },
];

#[derive(Serialize, JsonSchema)]
pub struct TemplateView {
    pub id: String,
    /// `agent`, `workflow` or `persona`.
    pub kind: String,
    pub title: String,
    pub description: String,
    pub needs: Vec<String>,
    pub yaml: String,
    /// An agent, workflow or persona with this name is already stored.
    pub installed: bool,
}

/// Example files take their name from the file; templates from their id.
fn named(t: &Template) -> String {
    if t.yaml.lines().any(|l| l.starts_with("name:")) {
        t.yaml.to_owned()
    } else {
        format!(
            "name: {}
{}",
            t.id, t.yaml
        )
    }
}

fn template_agent(t: &Template) -> Result<AgentSpec, String> {
    AgentSpec::parse(&named(t), "yaml")
}

fn template_workflow(t: &Template) -> Result<WorkflowSpec, String> {
    WorkflowSpec::parse(&named(t), "yaml")
}

fn template_persona(t: &Template) -> Result<PersonaSpec, String> {
    PersonaSpec::parse(&named(t), "yaml")
}

async fn exists(s: &AppState, kind: &str, name: &str) -> bool {
    let store = s.rt.store();
    match kind {
        "agent" => matches!(store.get_agent(name, None).await, Ok(Some(_))),
        "persona" => matches!(store.get_persona(name, None).await, Ok(Some(_))),
        _ => matches!(store.get_workflow(name, None).await, Ok(Some(_))),
    }
}

async fn list_templates(State(s): State<AppState>) -> Json<Vec<TemplateView>> {
    let mut out = Vec::new();
    for t in TEMPLATES {
        let description = match t.kind {
            "agent" => template_agent(t).ok().and_then(|a| a.description),
            "persona" => template_persona(t).ok().and_then(|p| p.description),
            _ => template_workflow(t).ok().and_then(|w| w.description),
        }
        .unwrap_or_default();
        out.push(TemplateView {
            id: t.id.into(),
            kind: t.kind.into(),
            title: t.title.into(),
            description,
            needs: t.needs.iter().map(|n| (*n).to_owned()).collect(),
            yaml: t.yaml.into(),
            installed: exists(&s, t.kind, t.id).await,
        });
    }
    Json(out)
}

#[derive(Serialize, JsonSchema)]
pub struct Installed {
    pub kind: String,
    pub name: String,
    /// Agents installed along with it because it uses them.
    pub also: Vec<String>,
}

/// Save a template (and the agents it uses, when missing). An agent or
/// workflow that already exists under the template's name is kept as is.
async fn install_template(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<(StatusCode, Json<Installed>)> {
    let t = TEMPLATES
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| ApiError::not_found(format!("template {id}")))?;
    let mut also = Vec::new();
    for need in t.needs {
        if !exists(&s, "agent", need).await {
            let dep = TEMPLATES
                .iter()
                .find(|d| d.id == *need && d.kind == "agent")
                .ok_or_else(|| ApiError::not_found(format!("template {need}")))?;
            let spec = template_agent(dep).map_err(ApiError::bad_request)?;
            s.rt.save_agent(&spec, Some("from template")).await?;
            also.push((*need).to_owned());
        }
    }
    if !exists(&s, t.kind, t.id).await {
        match t.kind {
            "agent" => {
                let spec = template_agent(t).map_err(ApiError::bad_request)?;
                s.rt.save_agent(&spec, Some("from template")).await?;
            }
            "persona" => {
                let spec = template_persona(t).map_err(ApiError::bad_request)?;
                s.rt.save_persona(&spec, Some("from template")).await?;
            }
            _ => {
                let spec = template_workflow(t).map_err(ApiError::bad_request)?;
                s.wf.save(&spec, Some("from template")).await?;
            }
        }
    }
    Ok((
        StatusCode::CREATED,
        Json(Installed {
            kind: t.kind.into(),
            name: t.id.into(),
            also,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts() {
        for h in [
            "localhost:8484",
            "127.0.0.1:8484",
            "[::1]:8484",
            "gai.localhost",
            "LOCALHOST",
        ] {
            assert!(is_loopback_host(h), "{h}");
        }
        for h in [
            "evil.com:8484",
            "192.168.1.5:8484",
            "localhost.evil.com",
            "[::2]:80",
        ] {
            assert!(!is_loopback_host(h), "{h}");
        }
    }

    #[test]
    fn templates_parse() {
        for t in TEMPLATES {
            match t.kind {
                "agent" => {
                    template_agent(t).unwrap();
                }
                "persona" => {
                    template_persona(t).unwrap();
                }
                _ => {
                    template_workflow(t).unwrap();
                }
            }
            for n in t.needs {
                assert!(TEMPLATES.iter().any(|d| d.id == *n), "{} needs {n}", t.id);
            }
        }
    }

    #[test]
    fn errors_in_plain_words() {
        assert!(plain_error("HTTP 401 Unauthorized: invalid x-api-key").contains("not accepted"));
        assert!(plain_error("429 rate limited").contains("busy"));
        assert_eq!(plain_error("something odd"), "something odd");
    }
}
