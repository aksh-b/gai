//! Apps: an agent or workflow published as a small form with its own link,
//! and as a REST endpoint with an input schema (the form's fields) and an
//! optional output schema. An app can have a key of its own, which runs
//! that app and nothing else, or be public to anyone with the link.

use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use gai::agent::RunRequest;
use gai::orchestra::WorkflowRequest;
use gai::store::{AppRecord, Run, RunStatus, keys};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::api::RunIds;
use crate::auth::{Principal, header_token};
use crate::error::{ApiError, ApiResult};
use crate::orgs::Tenant;

/// What an app's own keys start with.
pub const APP_KEY_PREFIX: &str = "gai_app_";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/apps", get(list).post(save))
        .route("/v1/apps/{name}", get(get_one).delete(delete))
        .route("/v1/apps/{name}/key", post(new_key).delete(remove_key))
        .route("/v1/apps/{name}/form", get(form))
        .route("/v1/apps/{name}/openapi.json", get(openapi))
        .route("/v1/apps/{name}/run", post(run))
        .route("/v1/apps/{name}/runs/{id}", get(result))
}

/// Whether a request uses an app (its form, endpoint or results) rather
/// than changing it. These also accept the app's own key, or nothing at
/// all for a public app.
pub fn is_app_use(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/v1/apps/") else {
        return false;
    };
    let parts: Vec<&str> = rest.split('/').collect();
    matches!(
        parts.as_slice(),
        [_, "form" | "run" | "openapi.json"] | [_, "runs", _]
    )
}

/// An app: what it runs, the form people fill in, and who may use it.
///
/// ```yaml
/// name: summarize
/// title: Summarize a page
/// agent: researcher
/// fields:
///   - { name: url, label: Web address }
///   - { name: length, type: select, options: [short, long], default: short }
/// prompt: "Read {{ url }} and write a {{ length }} summary."
/// public: true
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AppSpec {
    /// Letters, digits, `-` and `_`. It is part of the app's address.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The assistant it runs: `name`, `name@v3` or `name@prod`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Or the workflow it runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    /// The form, in order. Without fields, a workflow app asks for each of
    /// the workflow's inputs and an assistant app for one request.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<AppField>,
    /// An assistant app's task: a template over the fields, e.g. `Read
    /// {{ url }}`. By default the only field's value, or every field as a
    /// `Label: value` line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// A JSON Schema the result follows. An assistant is asked to answer
    /// in it; a workflow's result is checked against it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// Anyone with the link may use the app, without a key.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub public: bool,
    /// The button's text (default "Run").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submit_label: Option<String>,
}

/// One field of an app's form, and one property of its endpoint's input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AppField {
    /// Letters, digits and `_`, starting with a letter: the input's key.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(rename = "type")]
    pub kind: FieldKind,
    /// The choices of a `select`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// A field without a default must be filled in, unless this is false.
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// Shown under the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

impl Default for AppField {
    fn default() -> Self {
        Self {
            name: String::new(),
            label: None,
            kind: FieldKind::Text,
            options: Vec::new(),
            required: true,
            default: None,
            help: None,
            placeholder: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    /// One line of text.
    #[default]
    Text,
    /// Several lines.
    LongText,
    Number,
    /// A yes/no switch.
    Boolean,
    /// One of `options`.
    Select,
    /// Several lines of text, one item each; sent as a list.
    List,
}

/// An app as stored.
#[derive(Serialize, JsonSchema)]
pub struct AppView {
    pub name: String,
    pub spec: AppSpec,
    /// The start of the app's key, when it has one.
    pub key_prefix: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What the app's page needs: no names of assistants or workflows.
#[derive(Serialize, JsonSchema)]
pub struct AppForm {
    pub name: String,
    pub title: String,
    pub description: Option<String>,
    pub fields: Vec<AppField>,
    pub submit_label: String,
    /// The result is JSON (the app has an output schema), not text.
    pub structured: bool,
}

/// A new key for an app, shown once.
#[derive(Serialize, JsonSchema)]
pub struct AppKey {
    pub secret: String,
    pub prefix: String,
}

/// A run started through an app.
#[derive(Serialize, JsonSchema)]
pub struct AppResult {
    pub run_id: String,
    /// The run's status (`queued`, `running`, `awaiting_approval`,
    /// `completed`, `failed`…), or `invalid_output` when a finished
    /// result doesn't follow the app's output schema.
    pub status: String,
    /// Whether the run has ended.
    pub done: bool,
    /// The result: JSON following the output schema, or text.
    pub output: Option<Value>,
    pub error: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AppRunQuery {
    /// `wait` (default) answers with the result; `background` answers at
    /// once with the run's id, to poll at `/v1/apps/{name}/runs/{id}`.
    pub mode: Option<String>,
}

fn not_found(name: &str) -> ApiError {
    ApiError::not_found(format!("app {name}"))
}

fn view(r: AppRecord) -> ApiResult<AppView> {
    Ok(AppView {
        spec: serde_json::from_value(r.spec).map_err(|e| ApiError::bad_request(e.to_string()))?,
        name: r.name,
        key_prefix: r.key_prefix,
        created_at: r.created_at,
        updated_at: r.updated_at,
    })
}

async fn load(s: &AppState, name: &str) -> ApiResult<(AppSpec, AppRecord)> {
    let r =
        s.rt.store()
            .get_app(name)
            .await?
            .ok_or_else(|| not_found(name))?;
    let spec = serde_json::from_value(r.spec.clone())
        .map_err(|e| ApiError::bad_request(format!("app {name}: {e}")))?;
    Ok((spec, r))
}

async fn list(Tenant(s): Tenant) -> ApiResult<Json<Vec<AppView>>> {
    let apps = s.rt.store().list_apps().await?;
    Ok(Json(apps.into_iter().map(view).collect::<ApiResult<_>>()?))
}

async fn get_one(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<AppView>> {
    let (_, r) = load(&s, &name).await?;
    Ok(Json(view(r)?))
}

async fn save(
    Tenant(s): Tenant,
    Json(spec): Json<AppSpec>,
) -> ApiResult<(StatusCode, Json<AppView>)> {
    check(&s, &spec).await?;
    let store = s.rt.store();
    store
        .put_app(&spec.name, &serde_json::to_value(&spec).expect("spec"))
        .await?;
    let r = store
        .get_app(&spec.name)
        .await?
        .ok_or_else(|| not_found(&spec.name))?;
    Ok((StatusCode::CREATED, Json(view(r)?)))
}

async fn delete(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<StatusCode> {
    if s.rt.store().delete_app(&name).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found(&name))
    }
}

/// Make (or replace) the app's key. The old one stops working.
async fn new_key(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<AppKey>> {
    load(&s, &name).await?;
    let mut bytes = [0u8; 20];
    getrandom::fill(&mut bytes).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let secret = format!(
        "{APP_KEY_PREFIX}{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let prefix = secret[..APP_KEY_PREFIX.len() + 6].to_owned();
    s.rt.store()
        .set_app_key(&name, Some((&keys::hash(&secret), &prefix)))
        .await?;
    Ok(Json(AppKey { secret, prefix }))
}

async fn remove_key(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<StatusCode> {
    s.rt.store().set_app_key(&name, None).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Check an app before saving it: its name, fields, template, and that
/// what it runs exists and takes these fields.
async fn check(s: &AppState, spec: &AppSpec) -> ApiResult<()> {
    let bad = |m: String| Err(ApiError::bad_request(m));
    if spec.name.is_empty()
        || !spec
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return bad(format!(
            "app name {:?} may only use letters, digits, '-' and '_'",
            spec.name
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for f in &spec.fields {
        let ok = f.name.starts_with(|c: char| c.is_ascii_alphabetic())
            && f.name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !ok {
            return bad(format!(
                "field {:?}: use letters, digits and '_', starting with a letter",
                f.name
            ));
        }
        if !seen.insert(f.name.as_str()) {
            return bad(format!("field {} is listed twice", f.name));
        }
        if f.kind == FieldKind::Select && f.options.is_empty() {
            return bad(format!("field {}: a select needs options", f.name));
        }
        if let Some(d) = &f.default
            && let Err(e) = coerce(f, d)
        {
            return bad(format!("field {}: default: {e}", f.name));
        }
    }
    if let Some(schema) = &spec.output_schema
        && !schema.is_object()
    {
        return bad("output_schema must be a JSON Schema object".into());
    }
    match (&spec.agent, &spec.workflow) {
        (Some(agent), None) => {
            s.gai.find_agent(agent).await?;
            if let Some(p) = &spec.prompt {
                minijinja::Environment::new()
                    .template_from_str(p)
                    .map_err(|e| ApiError::bad_request(format!("prompt: {e}")))?;
            }
        }
        (None, Some(workflow)) => {
            if spec.prompt.is_some() {
                return bad(
                    "prompt is for assistant apps; a workflow gets the fields as its inputs".into(),
                );
            }
            let (w, _) = s.wf.resolve(workflow).await?;
            for f in &spec.fields {
                if !w.inputs.contains_key(&f.name) {
                    return bad(format!(
                        "field {}: workflow {workflow} has no input of that name",
                        f.name
                    ));
                }
            }
            if !spec.fields.is_empty() {
                for (k, i) in &w.inputs {
                    if i.default.is_none() && !seen.contains(k.as_str()) {
                        return bad(format!(
                            "workflow {workflow} needs the input {k}: add a field for it"
                        ));
                    }
                }
            }
        }
        _ => return bad("give one of agent or workflow".into()),
    }
    Ok(())
}

/// The fields people fill in: the app's own, or ones made from what it
/// runs.
async fn fields(s: &AppState, spec: &AppSpec) -> ApiResult<Vec<AppField>> {
    if !spec.fields.is_empty() {
        return Ok(spec.fields.clone());
    }
    let Some(workflow) = &spec.workflow else {
        return Ok(vec![AppField {
            name: "input".into(),
            label: Some("Your request".into()),
            kind: FieldKind::LongText,
            ..Default::default()
        }]);
    };
    let (w, _) = s.wf.resolve(workflow).await?;
    Ok(w.inputs
        .into_iter()
        .map(|(name, i)| AppField {
            label: Some(label_of(&name)),
            kind: match &i.default {
                Some(Value::Array(_)) => FieldKind::List,
                _ => FieldKind::Text,
            },
            required: i.default.is_none(),
            default: i.default,
            help: i.description,
            name,
            ..Default::default()
        })
        .collect())
}

/// `due_date` → `Due date`.
fn label_of(name: &str) -> String {
    let words = name.replace('_', " ");
    let mut c = words.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// A value for a field, converted to the field's type.
fn coerce(f: &AppField, v: &Value) -> Result<Value, String> {
    match (f.kind, v) {
        (FieldKind::Text | FieldKind::LongText, Value::String(_)) => Ok(v.clone()),
        (FieldKind::Text | FieldKind::LongText, Value::Number(n)) => Ok(json!(n.to_string())),
        (FieldKind::Number, Value::Number(_)) => Ok(v.clone()),
        (FieldKind::Number, Value::String(t)) => t
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(|n| serde_json::Number::from_f64(n).map(Value::Number))
            .ok_or_else(|| format!("{t:?} isn't a number")),
        (FieldKind::Boolean, Value::Bool(_)) => Ok(v.clone()),
        (FieldKind::Boolean, Value::String(t)) => match t.as_str() {
            "true" | "yes" | "on" => Ok(json!(true)),
            "false" | "no" | "off" | "" => Ok(json!(false)),
            _ => Err(format!("{t:?} isn't true or false")),
        },
        (FieldKind::Select, Value::String(t)) if f.options.contains(t) => Ok(v.clone()),
        (FieldKind::Select, _) => Err(format!("choose one of {}", f.options.join(", "))),
        (FieldKind::List, Value::Array(_)) => Ok(v.clone()),
        (FieldKind::List, Value::String(t)) => Ok(Value::Array(
            t.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(|l| json!(l))
                .collect(),
        )),
        _ => Err(format!("expected {}", kind_name(f.kind))),
    }
}

fn kind_name(k: FieldKind) -> &'static str {
    match k {
        FieldKind::Text | FieldKind::LongText | FieldKind::Select => "text",
        FieldKind::Number => "a number",
        FieldKind::Boolean => "true or false",
        FieldKind::List => "a list",
    }
}

/// The inputs a request gives, checked against the fields: unknown names
/// are refused, missing ones take their default, and values are converted
/// to their field's type.
pub fn check_inputs(
    fields: &[AppField],
    given: &Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    if let Some(k) = given.keys().find(|k| !fields.iter().any(|f| &f.name == *k)) {
        return Err(format!("unknown input {k}"));
    }
    let mut out = Map::new();
    for f in fields {
        let v = match given.get(&f.name) {
            Some(Value::Null) | None => None,
            Some(Value::String(t)) if t.trim().is_empty() && f.kind != FieldKind::Boolean => None,
            Some(v) => Some(v),
        };
        match (v, &f.default) {
            (Some(v), _) => {
                let v = coerce(f, v).map_err(|e| format!("{}: {e}", f.name))?;
                out.insert(f.name.clone(), v);
            }
            (None, Some(d)) => {
                out.insert(f.name.clone(), d.clone());
            }
            (None, None) if f.kind == FieldKind::Boolean => {
                out.insert(f.name.clone(), json!(false));
            }
            (None, None) if f.required => return Err(format!("{} is required", f.name)),
            (None, None) => {}
        }
    }
    Ok(out)
}

/// The JSON Schema of an app's input.
pub fn input_schema(fields: &[AppField]) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for f in fields {
        let mut p = match f.kind {
            FieldKind::Text | FieldKind::LongText => json!({"type": "string"}),
            FieldKind::Number => json!({"type": "number"}),
            FieldKind::Boolean => json!({"type": "boolean"}),
            FieldKind::Select => json!({"type": "string", "enum": f.options}),
            FieldKind::List => json!({"type": "array", "items": {"type": "string"}}),
        };
        if let Some(l) = &f.label {
            p["title"] = json!(l);
        }
        if let Some(h) = &f.help {
            p["description"] = json!(h);
        }
        if let Some(d) = &f.default {
            p["default"] = d.clone();
        }
        if f.required && f.default.is_none() && f.kind != FieldKind::Boolean {
            required.push(f.name.clone());
        }
        props.insert(f.name.clone(), p);
    }
    json!({"type": "object", "properties": props, "required": required, "additionalProperties": false})
}

/// An assistant app's task from the inputs.
fn render_prompt(
    spec: &AppSpec,
    fields: &[AppField],
    inputs: &Map<String, Value>,
) -> ApiResult<String> {
    if let Some(p) = &spec.prompt {
        return minijinja::Environment::new()
            .render_str(p, inputs)
            .map_err(|e| ApiError::bad_request(format!("prompt: {e}")));
    }
    let text = |v: &Value| match v {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    };
    if let [f] = fields {
        return Ok(inputs.get(&f.name).map(text).unwrap_or_default());
    }
    Ok(fields
        .iter()
        .filter_map(|f| {
            let v = inputs.get(&f.name)?;
            Some(format!(
                "{}: {}",
                f.label.clone().unwrap_or_else(|| label_of(&f.name)),
                text(v)
            ))
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// May this request use the app? Signed-in callers and keys with the
/// right scope were let in by authentication; anyone else needs the
/// app's key, or the app to be public.
fn admit(
    s: &AppState,
    p: Option<&Principal>,
    headers: &HeaderMap,
    spec: &AppSpec,
    r: &AppRecord,
) -> ApiResult<()> {
    if p.is_some() {
        return Ok(());
    }
    let key_ok = header_token(headers).is_some_and(|t| {
        t.starts_with(APP_KEY_PREFIX) && r.key_hash.as_deref() == Some(keys::hash(&t).as_str())
    });
    if !key_ok && !spec.public {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "this app needs its key: use the link you were given, or send `Authorization: Bearer <app key>`",
        ));
    }
    if let Some(limiter) = &s.auth.limiter
        && limiter
            .check_key(&format!("app:{}/{}", s.org.id, spec.name))
            .is_err()
    {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded",
        ));
    }
    if s.orgs.rate_limited(&s.org).is_some() {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded",
        ));
    }
    Ok(())
}

async fn form(
    Tenant(s): Tenant,
    p: Option<Extension<Principal>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> ApiResult<Json<AppForm>> {
    let (spec, r) = load(&s, &name).await?;
    admit(&s, p.as_deref(), &headers, &spec, &r)?;
    Ok(Json(AppForm {
        fields: fields(&s, &spec).await?,
        title: spec.title.clone().unwrap_or_else(|| label_of(&spec.name)),
        name: spec.name,
        description: spec.description,
        submit_label: spec.submit_label.unwrap_or_else(|| "Run".into()),
        structured: spec.output_schema.is_some(),
    }))
}

async fn run(
    Tenant(s): Tenant,
    p: Option<Extension<Principal>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(q): Query<AppRunQuery>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let (spec, r) = load(&s, &name).await?;
    let p = p.map(|Extension(p)| p);
    admit(&s, p.as_ref(), &headers, &spec, &r)?;
    match &p {
        Some(p) => p.check_budget()?,
        None => {
            if let Some(e) = s.orgs.over_budget(&s, &s.org).await {
                return Err(e);
            }
        }
    }
    let background = match q.mode.as_deref() {
        None | Some("wait") => false,
        Some("background") => true,
        Some(m) => return Err(ApiError::bad_request(format!("unknown mode {m}"))),
    };
    let Value::Object(given) = body else {
        return Err(ApiError::bad_request("send the inputs as a JSON object"));
    };
    let fields = fields(&s, &spec).await?;
    let inputs = check_inputs(&fields, &given).map_err(ApiError::bad_request)?;
    let mut metadata = Map::new();
    metadata.insert("app".into(), json!(spec.name));
    if let Some(p) = &p {
        p.stamp(&mut metadata);
    }

    let (run_id, session_id) = if let Some(agent) = &spec.agent {
        let (mut agent_spec, version) = s.gai.find_agent(agent).await?;
        if let Some(schema) = &spec.output_schema {
            agent_spec.output_schema = Some(schema.clone());
        }
        let mut req = RunRequest::new(agent_spec, render_prompt(&spec, &fields, &inputs)?);
        req.agent_version = version;
        req.metadata = metadata;
        if background {
            s.rt.enqueue(req).await?
        } else {
            let o = s.rt.start(req).await?.wait().await;
            (o.run_id, o.session_id)
        }
    } else {
        let workflow = spec.workflow.as_deref().unwrap_or_default();
        let (w, version) = s.wf.resolve(workflow).await?;
        let mut req = WorkflowRequest::new(w, inputs);
        req.version = version;
        req.metadata = metadata;
        if background {
            s.wf.enqueue(req).await?
        } else {
            let o = s.wf.run(req).await?;
            (o.run_id, o.session_id)
        }
    };
    if background {
        return Ok((StatusCode::ACCEPTED, Json(RunIds { run_id, session_id })).into_response());
    }
    let run =
        s.rt.store()
            .get_run(&run_id)
            .await?
            .ok_or_else(|| ApiError::not_found(format!("run {run_id}")))?;
    Ok(Json(result_of(&spec, run)).into_response())
}

/// A run started through the app, so far.
async fn result(
    Tenant(s): Tenant,
    p: Option<Extension<Principal>>,
    headers: HeaderMap,
    Path((name, id)): Path<(String, String)>,
) -> ApiResult<Json<AppResult>> {
    let (spec, r) = load(&s, &name).await?;
    admit(&s, p.as_deref(), &headers, &spec, &r)?;
    let run =
        s.rt.store()
            .get_run(&id)
            .await?
            .filter(|run| run.metadata["app"] == json!(name))
            .ok_or_else(|| ApiError::not_found(format!("run {id} of app {name}")))?;
    Ok(Json(result_of(&spec, run)))
}

fn result_of(spec: &AppSpec, run: Run) -> AppResult {
    let done = !matches!(
        run.status,
        RunStatus::Queued | RunStatus::Running | RunStatus::AwaitingApproval
    );
    let mut out = AppResult {
        run_id: run.id,
        status: run.status.as_str().into(),
        done,
        output: run.output,
        error: run.error,
    };
    if run.status == RunStatus::Completed
        && let Some(schema) = &spec.output_schema
    {
        let v = out.output.clone().unwrap_or(Value::Null);
        if let Err(e) = follows(schema, &v, "result") {
            out.status = "invalid_output".into();
            out.error = Some(format!("the result doesn't follow the output schema: {e}"));
        }
    }
    out
}

/// Check a value against the common parts of JSON Schema: `type`,
/// `enum`, `properties`, `required`, `additionalProperties: false` and
/// `items`.
pub fn follows(schema: &Value, v: &Value, at: &str) -> Result<(), String> {
    let types: Vec<&str> = match &schema["type"] {
        Value::String(t) => vec![t.as_str()],
        Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let is = |t: &str| match t {
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => v.as_f64().is_some_and(|n| n.fract() == 0.0),
        "boolean" => v.is_boolean(),
        "object" => v.is_object(),
        "array" => v.is_array(),
        "null" => v.is_null(),
        _ => true,
    };
    if !types.is_empty() && !types.iter().any(|t| is(t)) {
        return Err(format!("{at} should be {}", types.join(" or ")));
    }
    if let Some(choices) = schema["enum"].as_array()
        && !choices.contains(v)
    {
        return Err(format!(
            "{at} should be one of {}",
            Value::Array(choices.clone())
        ));
    }
    if let Value::Object(o) = v {
        let props = schema["properties"].as_object();
        for k in schema["required"].as_array().into_iter().flatten() {
            if let Some(k) = k.as_str()
                && !o.contains_key(k)
            {
                return Err(format!("{at}.{k} is missing"));
            }
        }
        for (k, x) in o {
            match props.and_then(|p| p.get(k)) {
                Some(p) => follows(p, x, &format!("{at}.{k}"))?,
                None if schema["additionalProperties"] == json!(false) => {
                    return Err(format!("{at}.{k} isn't expected"));
                }
                None => {}
            }
        }
    }
    if let (Value::Array(items), Some(item)) = (v, schema.get("items")) {
        for (i, x) in items.iter().enumerate() {
            follows(item, x, &format!("{at}[{i}]"))?;
        }
    }
    Ok(())
}

/// An OpenAPI document for the app's endpoint alone, to hand to whoever
/// calls it.
async fn openapi(
    Tenant(s): Tenant,
    p: Option<Extension<Principal>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    let (spec, r) = load(&s, &name).await?;
    admit(&s, p.as_deref(), &headers, &spec, &r)?;
    let fields = fields(&s, &spec).await?;
    let output = spec
        .output_schema
        .clone()
        .unwrap_or_else(|| json!({"type": "string"}));
    let result = json!({
        "type": "object",
        "required": ["run_id", "status", "done"],
        "properties": {
            "run_id": {"type": "string"},
            "status": {"type": "string", "description": "completed, failed, awaiting_approval… or invalid_output"},
            "done": {"type": "boolean"},
            "output": output,
            "error": {"type": ["string", "null"]},
        },
    });
    let title = spec.title.clone().unwrap_or_else(|| label_of(&spec.name));
    let run_path = format!("/v1/apps/{name}/run");
    let result_path = format!("/v1/apps/{name}/runs/{{id}}");
    let mut doc = json!({
        "openapi": "3.1.0",
        "info": {"title": title, "version": r.updated_at.format("%Y%m%d%H%M%S").to_string()},
        "paths": {
            run_path: {"post": {
                "summary": format!("Run {title}"),
                "parameters": [{"name": "mode", "in": "query", "schema": {"type": "string", "enum": ["wait", "background"]},
                    "description": "wait (default) answers with the result; background answers at once with a run id"}],
                "requestBody": {"required": true, "content": {"application/json": {"schema": input_schema(&fields)}}},
                "responses": {
                    "200": {"description": "The result", "content": {"application/json": {"schema": result}}},
                    "202": {"description": "Started in the background", "content": {"application/json": {"schema": {
                        "type": "object", "properties": {"run_id": {"type": "string"}, "session_id": {"type": "string"}}}}}},
                },
            }},
            result_path: {"get": {
                "summary": "A run started through this app",
                "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}],
                "responses": {"200": {"description": "The result so far", "content": {"application/json": {"schema": result}}}},
            }},
        },
        "components": {"securitySchemes": {"key": {"type": "http", "scheme": "bearer"}}},
    });
    if let Some(d) = &spec.description {
        doc["info"]["description"] = json!(d);
    }
    if !spec.public {
        doc["security"] = json!([{"key": []}]);
    }
    Ok(Json(doc))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, kind: FieldKind) -> AppField {
        AppField {
            name: name.into(),
            kind,
            ..Default::default()
        }
    }

    #[test]
    fn inputs_are_checked_and_converted() {
        let mut size = field("size", FieldKind::Select);
        size.options = vec!["short".into(), "long".into()];
        size.default = Some(json!("short"));
        let fields = [
            field("topic", FieldKind::Text),
            field("count", FieldKind::Number),
            field("draft", FieldKind::Boolean),
            size,
        ];
        let given = |v: Value| v.as_object().unwrap().clone();
        let got = check_inputs(&fields, &given(json!({"topic": "bees", "count": "3"}))).unwrap();
        assert_eq!(
            Value::Object(got),
            json!({"topic": "bees", "count": 3.0, "draft": false, "size": "short"})
        );
        let err = |v| check_inputs(&fields, &given(v)).unwrap_err();
        assert_eq!(err(json!({"count": 1})), "topic is required");
        assert_eq!(err(json!({"topic": " ", "count": 1})), "topic is required");
        assert!(err(json!({"topic": "a", "count": "lots"})).contains("isn't a number"));
        assert!(err(json!({"topic": "a", "count": 1, "size": "huge"})).contains("choose one of"));
        assert_eq!(
            err(json!({"topic": "a", "count": 1, "x": 1})),
            "unknown input x"
        );
        let list = [field("names", FieldKind::List)];
        let got = check_inputs(&list, &given(json!({"names": "ada\n\n grace "}))).unwrap();
        assert_eq!(got["names"], json!(["ada", "grace"]));
    }

    #[test]
    fn input_schema_lists_required_fields() {
        let mut opt = field("note", FieldKind::LongText);
        opt.required = false;
        let s = input_schema(&[field("topic", FieldKind::Text), opt]);
        assert_eq!(s["required"], json!(["topic"]));
        assert_eq!(s["properties"]["note"]["type"], "string");
        assert_eq!(s["additionalProperties"], false);
    }

    #[test]
    fn prompts_default_to_labelled_lines() {
        let spec = AppSpec::default();
        let mut url = field("web_address", FieldKind::Text);
        url.label = Some("Page".into());
        let fields = [url, field("tone", FieldKind::Text)];
        let inputs = json!({"web_address": "x.org", "tone": "dry"});
        let p = render_prompt(&spec, &fields, inputs.as_object().unwrap())
            .ok()
            .unwrap();
        assert_eq!(p, "Page: x.org\nTone: dry");
        let one = render_prompt(&spec, &fields[..1], inputs.as_object().unwrap())
            .ok()
            .unwrap();
        assert_eq!(one, "x.org");
        let spec = AppSpec {
            prompt: Some("Read {{ web_address }}, {{ tone }}.".into()),
            ..Default::default()
        };
        let p = render_prompt(&spec, &fields, inputs.as_object().unwrap())
            .ok()
            .unwrap();
        assert_eq!(p, "Read x.org, dry.");
    }

    #[test]
    fn outputs_are_checked_against_the_schema() {
        let schema = json!({
            "type": "object",
            "required": ["score"],
            "additionalProperties": false,
            "properties": {
                "score": {"type": "integer"},
                "tags": {"type": "array", "items": {"enum": ["a", "b"]}},
            },
        });
        assert!(follows(&schema, &json!({"score": 3, "tags": ["a"]}), "result").is_ok());
        let err = |v| follows(&schema, &v, "result").unwrap_err();
        assert_eq!(err(json!({})), "result.score is missing");
        assert_eq!(err(json!({"score": 1.5})), "result.score should be integer");
        assert!(
            err(json!({"score": 1, "tags": ["c"]})).starts_with("result.tags[0] should be one of")
        );
        assert_eq!(err(json!({"score": 1, "x": 0})), "result.x isn't expected");
        assert_eq!(err(json!("text")), "result should be object");
    }

    #[test]
    fn app_use_paths() {
        assert!(is_app_use("/v1/apps/intake/run"));
        assert!(is_app_use("/v1/apps/intake/form"));
        assert!(is_app_use("/v1/apps/intake/runs/run_1"));
        assert!(is_app_use("/v1/apps/intake/openapi.json"));
        assert!(!is_app_use("/v1/apps/intake"));
        assert!(!is_app_use("/v1/apps/intake/key"));
        assert!(!is_app_use("/v1/apps"));
    }
}
