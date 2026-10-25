//! Saving triggers, firing them, and the loops that do it on their own.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use gai_agent::{AgentError, RunRequest, Runtime};
use gai_orchestra::{WorkflowError, WorkflowRequest, Workflows};
use gai_store::{NewTriggerEvent, SharedStore, StoreError, TriggerRecord};
use hmac::{Hmac, Mac};
use notify::{EventKind, RecursiveMode, Watcher};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::Sha256;
use tokio::sync::{Notify, mpsc};

use crate::spec::{CatchUp, Source, Target, TriggerSpec, glob};

#[derive(Debug, thiserror::Error)]
pub enum TriggerError {
    #[error("invalid trigger: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error(transparent)]
    Workflow(#[from] WorkflowError),
}

/// What one firing did.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct Fired {
    pub event_id: i64,
    /// `started`, `skipped` or `failed`.
    pub status: String,
    pub run_id: Option<String>,
    pub session_id: Option<String>,
    pub error: Option<String>,
}

/// A time this late is "missed" rather than just due.
const GRACE: chrono::Duration = chrono::Duration::seconds(90);

#[derive(Clone)]
pub struct Triggers {
    inner: Arc<Inner>,
}

struct Inner {
    rt: Runtime,
    wf: Workflows,
    wake: Notify,
    /// Running file watchers by trigger id, with the spec they watch.
    watchers: Mutex<HashMap<String, (TriggerSpec, notify::RecommendedWatcher)>>,
    started: std::sync::atomic::AtomicBool,
}

impl Triggers {
    pub fn new(rt: Runtime, wf: Workflows) -> Self {
        Self {
            inner: Arc::new(Inner {
                rt,
                wf,
                wake: Notify::new(),
                watchers: Mutex::new(HashMap::new()),
                started: Default::default(),
            }),
        }
    }

    fn store(&self) -> &SharedStore {
        self.inner.rt.store()
    }

    pub fn spec_of(record: &TriggerRecord) -> Result<TriggerSpec, TriggerError> {
        serde_json::from_value(record.spec.clone())
            .map_err(|e| TriggerError::Invalid(format!("stored trigger {}: {e}", record.name)))
    }

    /// Check a spec, including that its target exists.
    pub async fn check(&self, spec: &TriggerSpec) -> Result<(), TriggerError> {
        spec.validate().map_err(TriggerError::Invalid)?;
        if let Source::File { path, .. } = &spec.source {
            self.watch_path(path).map_err(TriggerError::Invalid)?;
        }
        match &spec.target {
            Target::Agent(r) => {
                self.inner.rt.resolve_agent(r).await?;
            }
            Target::Workflow(r) => {
                self.inner.wf.resolve(r).await?;
            }
        }
        Ok(())
    }

    /// The folder a file trigger watches. A confined organization watches
    /// only inside its own data folder (relative paths start there).
    fn watch_path(&self, path: &str) -> Result<PathBuf, String> {
        let config = self.inner.rt.config();
        if !config.tools.confined {
            return Ok(PathBuf::from(path));
        }
        let root = config.data_dir.join("files");
        let p = std::path::Path::new(path);
        let rel = p.strip_prefix(&root).unwrap_or(p);
        if rel.components().any(|c| {
            !matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        }) {
            return Err(format!(
                "watch a folder inside {} (relative paths start there)",
                root.display()
            ));
        }
        Ok(root.join(rel))
    }

    /// Store a new trigger. Webhooks get a signing secret.
    pub async fn create(&self, spec: TriggerSpec) -> Result<TriggerRecord, TriggerError> {
        self.check(&spec).await?;
        let secret = matches!(spec.source, Source::Webhook {}).then(new_secret);
        let value = serde_json::to_value(&spec).expect("serializable spec");
        let rec = self
            .store()
            .create_trigger(&spec.name, spec.source.kind(), &value, secret.as_deref())
            .await?;
        if !spec.enabled {
            self.store().set_trigger_enabled(&rec.id, false).await?;
        }
        self.reschedule(&rec.id, &spec).await?;
        self.changed();
        self.get(&rec.id).await
    }

    /// Replace a trigger's spec. A schedule starts over from now.
    pub async fn update(&self, id: &str, spec: TriggerSpec) -> Result<TriggerRecord, TriggerError> {
        self.check(&spec).await?;
        let old = self.get(id).await?;
        let value = serde_json::to_value(&spec).expect("serializable spec");
        let rec = self
            .store()
            .update_trigger(&old.id, &spec.name, spec.source.kind(), &value)
            .await?;
        // A trigger that became a webhook needs a secret.
        if matches!(spec.source, Source::Webhook {}) && rec.secret.is_none() {
            return Err(TriggerError::Invalid(
                "a trigger cannot become a webhook; create a new one".into(),
            ));
        }
        self.reschedule(&rec.id, &spec).await?;
        self.changed();
        self.get(&rec.id).await
    }

    pub async fn get(&self, id_or_name: &str) -> Result<TriggerRecord, TriggerError> {
        self.store()
            .get_trigger(id_or_name)
            .await?
            .ok_or_else(|| TriggerError::NotFound(format!("trigger {id_or_name}")))
    }

    pub async fn delete(&self, id_or_name: &str) -> Result<(), TriggerError> {
        let rec = self.get(id_or_name).await?;
        self.store().delete_trigger(&rec.id).await?;
        self.changed();
        Ok(())
    }

    pub async fn set_enabled(
        &self,
        id_or_name: &str,
        enabled: bool,
    ) -> Result<TriggerRecord, TriggerError> {
        let rec = self.get(id_or_name).await?;
        self.store().set_trigger_enabled(&rec.id, enabled).await?;
        if enabled {
            self.reschedule(&rec.id, &Self::spec_of(&rec)?).await?;
        }
        self.changed();
        self.get(&rec.id).await
    }

    async fn reschedule(&self, id: &str, spec: &TriggerSpec) -> Result<(), TriggerError> {
        let next = spec.next_after(Utc::now()).map_err(TriggerError::Invalid)?;
        self.store().set_trigger_next(id, next).await?;
        Ok(())
    }

    /// Something changed: re-read schedules and file watchers.
    fn changed(&self) {
        self.inner.wake.notify_one();
    }

    /// Fire a trigger now. `wait` runs the target here and waits for it
    /// (for the CLI); otherwise the run goes to the background queue.
    pub async fn fire(
        &self,
        rec: &TriggerRecord,
        reason: &str,
        payload: Option<Value>,
        wait: bool,
    ) -> Result<Fired, TriggerError> {
        let spec = Self::spec_of(rec)?;
        let store = self.store();
        let event =
            |status: &str, run: Option<(&str, &str)>, error: Option<String>| NewTriggerEvent {
                trigger_id: rec.id.clone(),
                reason: reason.to_owned(),
                payload: payload.clone(),
                run_id: run.map(|r| r.0.to_owned()),
                status: status.to_owned(),
                error,
            };
        if spec.skip_if_running
            && let Some(prev) = store
                .list_trigger_events(&rec.id, 5)
                .await?
                .into_iter()
                .find_map(|e| e.run_id)
            && let Some(run) = store.get_run(&prev).await?
            && !run.status.is_terminal()
        {
            let why = format!("run {prev} from the last firing has not finished");
            let id = store
                .add_trigger_event(event("skipped", None, Some(why.clone())))
                .await?;
            return Ok(Fired {
                event_id: id,
                status: "skipped".into(),
                run_id: None,
                session_id: None,
                error: Some(why),
            });
        }
        store.touch_trigger(&rec.id, Utc::now()).await?;
        let started = self.start(rec, &spec, reason, payload.as_ref(), wait).await;
        let (status, run, error) = match &started {
            Ok((r, s)) => ("started", Some((r.as_str(), s.as_str())), None),
            Err(e) => ("failed", None, Some(e.to_string())),
        };
        let id = store
            .add_trigger_event(event(status, run, error.clone()))
            .await?;
        if let Err(e) = &started {
            tracing::warn!(trigger = %rec.name, error = %e, "trigger could not start its run");
        }
        Ok(Fired {
            event_id: id,
            status: status.into(),
            run_id: run.map(|r| r.0.to_owned()),
            session_id: run.map(|r| r.1.to_owned()),
            error,
        })
    }

    async fn start(
        &self,
        rec: &TriggerRecord,
        spec: &TriggerSpec,
        reason: &str,
        payload: Option<&Value>,
        wait: bool,
    ) -> Result<(String, String), TriggerError> {
        let ctx = context(rec, reason, payload);
        let mut meta = Map::new();
        meta.insert("source".into(), json!("trigger"));
        meta.insert("trigger_id".into(), json!(rec.id));
        meta.insert("trigger".into(), json!(rec.name));
        meta.insert("trigger_reason".into(), json!(reason));
        match &spec.target {
            Target::Agent(r) => {
                let template = spec
                    .input
                    .clone()
                    .unwrap_or_else(|| default_input(&spec.source).to_owned());
                let input = render(&template, &ctx).map_err(TriggerError::Invalid)?;
                let (agent, version) = self.inner.rt.resolve_agent(r).await?;
                let mut req = RunRequest::new(agent, input);
                req.agent_version = version;
                req.auto_approve = spec.auto_approve;
                req.metadata = meta;
                if wait {
                    let h = self.inner.rt.start(req).await?;
                    let ids = (h.run_id.clone(), h.session_id.clone());
                    h.wait().await;
                    Ok(ids)
                } else {
                    Ok(self.inner.rt.enqueue(req).await?)
                }
            }
            Target::Workflow(r) => {
                let (wf, version) = self.inner.wf.resolve(r).await?;
                let mut inputs = Map::new();
                for (k, v) in &spec.inputs {
                    inputs.insert(
                        k.clone(),
                        render_value(v, &ctx).map_err(TriggerError::Invalid)?,
                    );
                }
                // Webhook bodies that are objects fill inputs not set above.
                if let Some(Value::Object(p)) = payload
                    && matches!(spec.source, Source::Webhook {})
                {
                    for (k, v) in p {
                        if wf.inputs.contains_key(k) && !inputs.contains_key(k) {
                            inputs.insert(k.clone(), v.clone());
                        }
                    }
                }
                let mut req = WorkflowRequest::new(wf, inputs);
                req.version = version;
                req.auto_approve = spec.auto_approve;
                req.metadata = meta;
                if wait {
                    let h = self.inner.wf.start(req).await?;
                    let ids = (h.run_id.clone(), h.session_id.clone());
                    h.wait().await;
                    Ok(ids)
                } else {
                    Ok(self.inner.wf.enqueue(req).await?)
                }
            }
        }
    }

    /// Start the schedule loop and file watchers, for as long as the process
    /// lives. Calling it again does nothing.
    pub fn start_loops(&self) {
        if self
            .inner
            .started
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                let sleep = match me.tick().await {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!(error = %e, "trigger loop");
                        Duration::from_secs(30)
                    }
                };
                me.sync_watchers().await;
                let _ = tokio::time::timeout(sleep, me.inner.wake.notified()).await;
            }
        });
    }

    /// Fire what is due; return how long to sleep.
    pub async fn tick(&self) -> Result<Duration, TriggerError> {
        let now = Utc::now();
        let mut soonest = now + chrono::Duration::seconds(60);
        for rec in self.store().list_triggers().await? {
            if !rec.enabled || rec.kind != "cron" {
                continue;
            }
            let Ok(spec) = Self::spec_of(&rec) else {
                continue;
            };
            let Some(due) = rec.next_fire_at else {
                self.reschedule(&rec.id, &spec).await?;
                continue;
            };
            if due > now {
                soonest = soonest.min(due);
                continue;
            }
            let next = spec.next_after(now).map_err(TriggerError::Invalid)?;
            // Only the process that moves the schedule on fires it.
            if !self
                .store()
                .advance_trigger(&rec.id, due, next, now)
                .await?
            {
                continue;
            }
            if let Some(n) = next {
                soonest = soonest.min(n);
            }
            let missed = now - due > GRACE;
            let catch_up = match &spec.source {
                Source::Cron { catch_up, .. } => *catch_up,
                _ => CatchUp::Once,
            };
            if missed && catch_up == CatchUp::Skip {
                self.store()
                    .add_trigger_event(NewTriggerEvent {
                        trigger_id: rec.id.clone(),
                        reason: "missed".into(),
                        payload: Some(json!({"due": due})),
                        status: "skipped".into(),
                        error: Some("missed while gai was not running; catch_up is skip".into()),
                        ..Default::default()
                    })
                    .await?;
                continue;
            }
            let reason = if missed { "missed" } else { "schedule" };
            self.fire(&rec, reason, Some(json!({"due": due})), false)
                .await?;
        }
        Ok((soonest - Utc::now())
            .to_std()
            .unwrap_or(Duration::from_millis(200))
            .max(Duration::from_millis(200)))
    }

    /// Run a watcher for each enabled file trigger, and only those.
    async fn sync_watchers(&self) {
        let Ok(all) = self.store().list_triggers().await else {
            return;
        };
        let wanted: Vec<(TriggerRecord, TriggerSpec)> = all
            .into_iter()
            .filter(|r| r.enabled && r.kind == "file")
            .filter_map(|r| Self::spec_of(&r).ok().map(|s| (r, s)))
            .collect();
        let mut watchers = self.inner.watchers.lock().expect("watchers lock");
        watchers.retain(|id, (spec, _)| wanted.iter().any(|(r, s)| &r.id == id && s == spec));
        for (rec, spec) in wanted {
            if watchers.contains_key(&rec.id) {
                continue;
            }
            match self.watch(&rec, &spec) {
                Ok(w) => {
                    watchers.insert(rec.id.clone(), (spec, w));
                }
                Err(e) => tracing::warn!(trigger = %rec.name, error = %e, "cannot watch files"),
            }
        }
    }

    fn watch(
        &self,
        rec: &TriggerRecord,
        spec: &TriggerSpec,
    ) -> Result<notify::RecommendedWatcher, String> {
        let Source::File {
            path,
            pattern,
            debounce_ms,
        } = &spec.source
        else {
            return Err("not a file trigger".into());
        };
        let root = self.watch_path(path)?;
        let (tx, mut rx) = mpsc::unbounded_channel::<(PathBuf, &'static str)>();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            let what = match ev.kind {
                EventKind::Create(_) => "created",
                EventKind::Modify(_) => "modified",
                EventKind::Remove(_) => "removed",
                _ => return,
            };
            for p in ev.paths {
                let _ = tx.send((p, what));
            }
        })
        .map_err(|e| e.to_string())?;
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|e| format!("{}: {e}", root.display()))?;

        let me = self.clone();
        let id = rec.id.clone();
        let pattern = pattern.clone();
        let debounce = Duration::from_millis((*debounce_ms).max(50));
        tokio::spawn(async move {
            // Changes to one file come in bursts; fire once it settles.
            let mut pending: HashMap<PathBuf, &'static str> = HashMap::new();
            loop {
                let next = if pending.is_empty() {
                    rx.recv().await
                } else {
                    match tokio::time::timeout(debounce, rx.recv()).await {
                        Ok(v) => v,
                        Err(_) => {
                            for (p, what) in std::mem::take(&mut pending) {
                                me.file_changed(&id, &p, what).await;
                            }
                            continue;
                        }
                    }
                };
                let Some((p, what)) = next else { break };
                if p.is_dir() {
                    continue;
                }
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                if pattern.as_deref().is_some_and(|g| !glob(g, name)) {
                    continue;
                }
                // A file made and then written is still "created".
                let entry = pending.entry(p).or_insert(what);
                if what == "removed" {
                    *entry = "removed";
                }
            }
        });
        Ok(watcher)
    }

    async fn file_changed(&self, id: &str, path: &Path, what: &str) {
        let Ok(rec) = self.get(id).await else { return };
        if !rec.enabled {
            return;
        }
        let payload = json!({"path": path.display().to_string(), "event": what});
        if let Err(e) = self.fire(&rec, "file", Some(payload), false).await {
            tracing::warn!(trigger = %rec.name, error = %e, "file trigger");
        }
    }
}

fn new_secret() -> String {
    let mut b = [0u8; 24];
    getrandom::fill(&mut b).expect("system randomness");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Check a webhook request: an HMAC-SHA256 signature of the body
/// (`sha256=<hex>`, as GitHub sends in `X-Hub-Signature-256`), or the secret
/// itself as a token. Both compared in constant time.
pub fn verify_signature(
    secret: &str,
    body: &[u8],
    signature: Option<&str>,
    token: Option<&str>,
) -> bool {
    if let Some(sig) = signature {
        let hex = sig.trim().strip_prefix("sha256=").unwrap_or(sig.trim());
        let Some(expected) = from_hex(hex) else {
            return false;
        };
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("any key length");
        mac.update(body);
        return mac.verify_slice(&expected).is_ok();
    }
    if let Some(t) = token {
        let (a, b) = (t.trim().as_bytes(), secret.as_bytes());
        return a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0;
    }
    false
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn default_input(source: &Source) -> &'static str {
    match source {
        Source::Cron { .. } => "It is {{ now }}. Do your scheduled work.",
        Source::Webhook {} => "A webhook arrived. Its body:\n\n{{ payload | tojson }}",
        Source::File { .. } => {
            "The file {{ path }} was {{ event }}.{% if content %}\n\nIts contents:\n\n{{ content }}{% endif %}"
        }
    }
}

/// Variables templates see.
fn context(rec: &TriggerRecord, reason: &str, payload: Option<&Value>) -> Value {
    let now = Utc::now();
    let mut ctx = json!({
        "trigger": rec.name,
        "reason": reason,
        "now": now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "date": now.format("%Y-%m-%d").to_string(),
        "payload": payload.cloned().unwrap_or(Value::Null),
    });
    if let Some(p) = payload
        && let Some(path) = p["path"].as_str()
    {
        ctx["path"] = json!(path);
        ctx["event"] = p["event"].clone();
        // Small text files come along, so agents can act on them directly.
        if let Ok(meta) = std::fs::metadata(path)
            && meta.len() <= 100_000
            && let Ok(text) = std::fs::read_to_string(path)
        {
            ctx["content"] = json!(text);
        }
    }
    ctx
}

fn render(template: &str, ctx: &Value) -> Result<String, String> {
    let env = minijinja::Environment::new();
    env.render_str(template, ctx)
        .map_err(|e| format!("template: {e}"))
}

/// Strings render as templates; `"{{ expr }}"` alone keeps the value's type.
fn render_value(v: &Value, ctx: &Value) -> Result<Value, String> {
    match v {
        Value::String(s) => {
            let t = s.trim();
            if let Some(expr) = t
                .strip_prefix("{{")
                .and_then(|r| r.strip_suffix("}}"))
                .filter(|e| !e.contains("{{") && !e.contains("}}"))
            {
                let env = minijinja::Environment::new();
                let e = env
                    .compile_expression(expr.trim())
                    .map_err(|e| format!("template: {e}"))?;
                let out = e.eval(ctx).map_err(|e| format!("template: {e}"))?;
                return serde_json::to_value(&out).map_err(|e| e.to_string());
            }
            render(s, ctx).map(Value::String)
        }
        Value::Array(a) => a
            .iter()
            .map(|x| render_value(x, ctx))
            .collect::<Result<_, _>>()
            .map(Value::Array),
        Value::Object(o) => o
            .iter()
            .map(|(k, x)| Ok((k.clone(), render_value(x, ctx)?)))
            .collect::<Result<Map<_, _>, String>>()
            .map(Value::Object),
        other => Ok(other.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_and_tokens() {
        let mut mac = Hmac::<Sha256>::new_from_slice(b"s3cret").unwrap();
        mac.update(b"{\"a\":1}");
        let sig: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(verify_signature(
            "s3cret",
            b"{\"a\":1}",
            Some(&format!("sha256={sig}")),
            None
        ));
        assert!(verify_signature("s3cret", b"{\"a\":1}", Some(&sig), None));
        assert!(!verify_signature(
            "s3cret",
            b"{\"a\":2}",
            Some(&format!("sha256={sig}")),
            None
        ));
        assert!(!verify_signature("s3cret", b"x", Some("sha256=zz"), None));
        assert!(verify_signature("s3cret", b"x", None, Some("s3cret")));
        assert!(!verify_signature("s3cret", b"x", None, Some("s3cre")));
        assert!(!verify_signature("s3cret", b"x", None, None));
    }

    #[test]
    fn values_keep_types_when_they_are_one_expression() {
        let ctx = json!({"payload": {"n": 3, "who": "ada"}, "date": "2026-10-15"});
        assert_eq!(
            render_value(&json!("{{ payload.n }}"), &ctx).unwrap(),
            json!(3)
        );
        assert_eq!(
            render_value(&json!("hi {{ payload.who }} on {{ date }}"), &ctx).unwrap(),
            json!("hi ada on 2026-10-15")
        );
        assert_eq!(
            render_value(&json!({"list": ["{{ payload }}"]}), &ctx).unwrap(),
            json!({"list": [{"n": 3, "who": "ada"}]})
        );
    }
}
