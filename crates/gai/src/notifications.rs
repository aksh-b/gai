//! Notifications: rules that say who hears about approvals, finished and
//! failed runs, and dashboard alerts, by email, Slack, webhook or web push.
//!
//! ```yaml
//! name: on-call
//! on: [approval, run_failed]
//! agents: [triage]          # all runs when empty
//! email: [ops@example.com]
//! slack_env: OPS_SLACK_WEBHOOK
//! push: true                # every browser that turned notifications on
//! ```
//!
//! Rules and push subscriptions are settings (`notify.rule.<name>`,
//! `notify.push.<id>`), so they live in each organization's database.

use std::sync::Arc;

use gai_agent::PendingApproval;
use gai_agent::{RunOutcome, Runtime};
use gai_store::RunStatus;
use gai_store::SharedStore;
use gai_tools::notify::{Delivery, Notice, Notifier};
use gai_tools::webpush::{self, PushError, PushSubscription, Vapid};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

use crate::Error;

const RULE_PREFIX: &str = "notify.rule.";
const PUSH_PREFIX: &str = "notify.push.";
const VAPID_SETTING: &str = "notify.vapid";

/// Something people can hear about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotifyEvent {
    /// A tool call or workflow step waits for someone to approve it.
    Approval,
    /// A run ended in failure.
    RunFailed,
    /// A run completed.
    RunFinished,
    /// A dashboard KPI crossed its limit.
    Alert,
    /// An agent suggested something to do.
    Suggestion,
}

/// Who hears about what.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationRule {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub on: Vec<NotifyEvent>,
    /// Only runs of these agents or workflows; every run when empty. Alerts
    /// match by dashboard name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
    #[serde(flatten)]
    pub deliver: Delivery,
    /// Also to every browser subscribed to web push.
    #[serde(default)]
    pub push: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl NotificationRule {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty()
            || !self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("a rule's name is letters, digits, - and _".into());
        }
        if self.on.is_empty() {
            return Err("say which events the rule is `on`".into());
        }
        if self.deliver.is_empty() && !self.push {
            return Err("send it somewhere: email, slack, webhook or push".into());
        }
        self.deliver.validate()
    }

    fn matches(&self, e: &Event) -> bool {
        self.enabled
            && self.on.contains(&e.kind)
            && (self.agents.is_empty()
                || e.source.as_ref().is_some_and(|s| self.agents.contains(s)))
    }
}

/// A browser that turned notifications on.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PushDevice {
    pub id: String,
    #[serde(flatten)]
    pub subscription: PushSubscription,
    /// Who subscribed (user id or key id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// What happened.
#[derive(Clone, Debug)]
pub struct Event {
    pub kind: NotifyEvent,
    /// The agent, workflow or dashboard it is about.
    pub source: Option<String>,
    pub subject: String,
    pub text: String,
    /// Dashboard path (`#/runs/<id>`), made absolute with `dashboard_url`.
    pub link: Option<String>,
    pub meta: Value,
}

/// One delivery attempt, as `send` reports it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct Sent {
    pub rule: String,
    pub to: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Rules, push subscriptions and sending.
#[derive(Clone)]
pub struct Notifications {
    store: SharedStore,
    notifier: Notifier,
    vapid: Arc<OnceCell<Arc<Vapid>>>,
}

impl Notifications {
    pub fn new(store: SharedStore, notifier: Notifier) -> Self {
        Self {
            store,
            notifier,
            vapid: Arc::default(),
        }
    }

    pub fn parse(value: Value) -> Result<NotificationRule, Error> {
        let rule: NotificationRule = serde_json::from_value(value)
            .map_err(|e| Error::Invalid(format!("notification rule: {e}")))?;
        rule.validate().map_err(Error::Invalid)?;
        Ok(rule)
    }

    pub async fn rules(&self) -> Result<Vec<NotificationRule>, Error> {
        let mut out = Vec::new();
        for (name, v) in self.store.list_settings(RULE_PREFIX).await? {
            match serde_json::from_str(&v) {
                Ok(r) => out.push(r),
                Err(e) => {
                    tracing::warn!(setting = %name, error = %e, "ignoring a bad notification rule")
                }
            }
        }
        Ok(out)
    }

    pub async fn rule(&self, name: &str) -> Result<NotificationRule, Error> {
        let v = self
            .store
            .get_setting(&format!("{RULE_PREFIX}{name}"))
            .await?
            .ok_or_else(|| Error::NotFound(format!("no notification rule {name}")))?;
        serde_json::from_str(&v).map_err(|e| Error::Invalid(e.to_string()))
    }

    /// Save a rule, replacing one of the same name.
    pub async fn save_rule(&self, rule: &NotificationRule) -> Result<(), Error> {
        rule.validate().map_err(Error::Invalid)?;
        let v = serde_json::to_string(rule).expect("rule");
        self.store
            .put_setting(&format!("{RULE_PREFIX}{}", rule.name), &v)
            .await?;
        Ok(())
    }

    pub async fn delete_rule(&self, name: &str) -> Result<bool, Error> {
        Ok(self
            .store
            .delete_setting(&format!("{RULE_PREFIX}{name}"))
            .await?)
    }

    /// The server's web push key: from `[notify] push_key_env`, else made
    /// once and kept.
    pub async fn vapid(&self) -> Result<Arc<Vapid>, Error> {
        self.vapid
            .get_or_try_init(|| async {
                let s = self.notifier.settings();
                let subject = s.push_subject();
                if let Some(var) = &s.push_key_env {
                    let key = std::env::var(var)
                        .map_err(|_| Error::Invalid(format!("set {var} (the web push key)")))?;
                    return Ok(Arc::new(
                        Vapid::from_base64(&key, &subject).map_err(Error::Invalid)?,
                    ));
                }
                if let Some(key) = self.store.get_setting(VAPID_SETTING).await? {
                    return Ok(Arc::new(
                        Vapid::from_base64(&key, &subject).map_err(Error::Invalid)?,
                    ));
                }
                let v = Vapid::generate(&subject).map_err(Error::Invalid)?;
                self.store
                    .put_setting(VAPID_SETTING, &v.to_base64())
                    .await?;
                Ok(Arc::new(v))
            })
            .await
            .cloned()
    }

    /// Remember a browser; subscribing the same endpoint again replaces it.
    pub async fn subscribe(
        &self,
        subscription: PushSubscription,
        owner: Option<String>,
    ) -> Result<PushDevice, Error> {
        subscription.validate().map_err(Error::Invalid)?;
        let device = PushDevice {
            id: device_id(&subscription.endpoint),
            subscription,
            owner,
            created_at: chrono::Utc::now(),
        };
        self.store
            .put_setting(
                &format!("{PUSH_PREFIX}{}", device.id),
                &serde_json::to_string(&device).expect("device"),
            )
            .await?;
        Ok(device)
    }

    /// Forget a browser, by its id or endpoint.
    pub async fn unsubscribe(&self, id_or_endpoint: &str) -> Result<bool, Error> {
        let id = if id_or_endpoint.contains("://") {
            device_id(id_or_endpoint)
        } else {
            id_or_endpoint.to_owned()
        };
        Ok(self
            .store
            .delete_setting(&format!("{PUSH_PREFIX}{id}"))
            .await?)
    }

    pub async fn devices(&self) -> Result<Vec<PushDevice>, Error> {
        Ok(self
            .store
            .list_settings(PUSH_PREFIX)
            .await?
            .into_iter()
            .filter_map(|(_, v)| serde_json::from_str(&v).ok())
            .collect())
    }

    /// Send `event` to every rule that wants it.
    pub async fn dispatch(&self, event: &Event) -> Vec<Sent> {
        let rules = match self.rules().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "could not read notification rules");
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for rule in rules.iter().filter(|r| r.matches(event)) {
            out.push(self.send(rule, event).await);
        }
        out
    }

    /// Send `event` through one rule (also used to test a rule).
    pub async fn send(&self, rule: &NotificationRule, event: &Event) -> Sent {
        let url = event.link.as_ref().map(|l| self.absolute(l));
        let mut meta = json!({"kind": event.kind, "rule": rule.name});
        if let Value::Object(m) = &event.meta {
            for (k, v) in m {
                meta[k] = v.clone();
            }
        }
        if let Some(u) = &url {
            meta["url"] = json!(u);
        }
        let mut text = event.text.clone();
        if let Some(u) = &url
            && rule.deliver.slack.is_none()
            && rule.deliver.slack_env.is_none()
        {
            text.push_str(&format!("\n\n{u}"));
        }
        let notice = Notice {
            subject: event.subject.clone(),
            text,
            attachments: Vec::new(),
            meta,
        };
        let mut sent = Sent {
            rule: rule.name.clone(),
            to: Vec::new(),
            error: None,
        };
        if !rule.deliver.is_empty() {
            match self.notifier.send(&rule.deliver, &notice).await {
                Ok(to) => sent.to.extend(to),
                Err(e) => sent.error = Some(e),
            }
        }
        if rule.push {
            match self.push(event, url.as_deref()).await {
                Ok(0) => {}
                Ok(n) => sent.to.push(format!("web push to {n} browser(s)")),
                Err(e) => sent.error = sent.error.or(Some(e)),
            }
        }
        if let Some(e) = &sent.error {
            tracing::warn!(rule = %rule.name, error = %e, "a notification was not sent");
        }
        sent
    }

    /// Push to every subscribed browser; how many got it. Browsers the push
    /// service says are gone are forgotten.
    async fn push(&self, event: &Event, url: Option<&str>) -> Result<usize, String> {
        let devices = self.devices().await.map_err(|e| e.to_string())?;
        if devices.is_empty() {
            return Ok(0);
        }
        let vapid = self.vapid().await.map_err(|e| e.to_string())?;
        let mut body: String = event.text.chars().take(600).collect();
        if body.len() < event.text.len() {
            body.push('…');
        }
        let payload = json!({
            "title": event.subject,
            "body": body,
            "url": url,
            "tag": event.meta.get("run_id").or(event.meta.get("dashboard")),
            "kind": event.kind,
        })
        .to_string();
        let (mut ok, mut last_err) = (0, None);
        for d in devices {
            match webpush::send(
                self.notifier.http(),
                &vapid,
                &d.subscription,
                payload.as_bytes(),
            )
            .await
            {
                Ok(()) => ok += 1,
                Err(PushError::Gone) => {
                    let _ = self.unsubscribe(&d.id).await;
                }
                Err(PushError::Failed(e)) => last_err = Some(e),
            }
        }
        match (ok, last_err) {
            (0, Some(e)) => Err(e),
            (n, _) => Ok(n),
        }
    }

    fn absolute(&self, link: &str) -> String {
        match &self.notifier.settings().dashboard_url {
            Some(base) => format!(
                "{}/{}",
                base.trim_end_matches('/'),
                link.trim_start_matches('/')
            ),
            None => link.to_owned(),
        }
    }

    /// Listen for approvals and finished runs on `rt`.
    pub fn attach(&self, rt: &Runtime) {
        let me = self.clone();
        let store = rt.store().clone();
        rt.set_finish_hook(
            "notifications",
            Arc::new(move |o: RunOutcome| {
                let (me, store) = (me.clone(), store.clone());
                Box::pin(async move {
                    if let Some(e) = run_event(&store, &o).await {
                        me.dispatch(&e).await;
                    }
                })
            }),
        );
        let mut requested = rt.approvals().watch();
        let me = self.clone();
        let store = rt.store().clone();
        tokio::spawn(async move {
            loop {
                match requested.recv().await {
                    Ok(p) => {
                        let e = approval_event(&store, &p).await;
                        let me = me.clone();
                        tokio::spawn(async move { me.dispatch(&e).await });
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(missed = n, "approval notifications fell behind");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// Alerts from dashboards, as events.
    pub fn alert_hook(&self) -> gai_data::dashboards::AlertHook {
        let me = self.clone();
        Arc::new(move |n: Notice| {
            let me = me.clone();
            Box::pin(async move {
                let dashboard = n.meta["dashboard"].as_str().map(str::to_owned);
                let e = Event {
                    kind: NotifyEvent::Alert,
                    link: dashboard.as_ref().map(|d| format!("#/dashboards/{d}")),
                    source: dashboard,
                    subject: n.subject,
                    text: n.text,
                    meta: n.meta,
                };
                me.dispatch(&e).await;
            })
        })
    }
}

fn device_id(endpoint: &str) -> String {
    let h = Sha256::digest(endpoint.as_bytes());
    h[..8].iter().map(|b| format!("{b:02x}")).collect()
}

fn short(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if out.len() < s.len() {
        out.push('…');
    }
    out
}

/// The event for a finished run: top-level runs only, not evals.
async fn run_event(store: &SharedStore, o: &RunOutcome) -> Option<Event> {
    let run = store.get_run(&o.run_id).await.ok().flatten()?;
    if run.parent_run_id.is_some() || run.metadata["kind"] == "eval" {
        return None;
    }
    let what = if run.metadata["kind"] == "workflow" {
        "Workflow"
    } else {
        "Run"
    };
    let (kind, subject, text) = match o.status {
        RunStatus::Completed => (
            NotifyEvent::RunFinished,
            format!("{what} finished: {}", run.agent),
            short(&o.text, 1500),
        ),
        RunStatus::Failed => (
            NotifyEvent::RunFailed,
            format!("{what} failed: {}", run.agent),
            o.error.clone().unwrap_or_else(|| "It failed.".into()),
        ),
        _ => return None,
    };
    Some(Event {
        kind,
        source: Some(run.agent.clone()),
        subject,
        text,
        link: Some(format!("#/runs/{}", run.id)),
        meta: json!({"run_id": run.id, "agent": run.agent, "cost": o.cost}),
    })
}

async fn approval_event(store: &SharedStore, p: &PendingApproval) -> Event {
    let agent = store
        .get_run(&p.run_id)
        .await
        .ok()
        .flatten()
        .map(|r| r.agent);
    let input = serde_json::to_string_pretty(&p.input).unwrap_or_default();
    Event {
        kind: NotifyEvent::Approval,
        subject: format!(
            "Approval needed: {} in {}",
            p.tool,
            agent.as_deref().unwrap_or("a run")
        ),
        text: format!("{} wants to run with:\n{}", p.tool, short(&input, 1200)),
        source: agent.clone(),
        link: Some(format!("#/runs/{}", p.run_id)),
        meta: json!({"run_id": p.run_id, "call_id": p.call_id, "tool": p.tool, "agent": agent}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(yaml: &str) -> Result<NotificationRule, Error> {
        Notifications::parse(serde_yaml::from_str(yaml).unwrap())
    }

    #[test]
    fn rules_are_checked() {
        let r = rule("name: oncall\non: [approval, run_failed]\nagents: [triage]\nslack: https://hooks.slack.com/services/x\npush: true\n").unwrap();
        assert!(r.enabled && r.push);
        assert_eq!(
            r.deliver.slack.as_deref(),
            Some("https://hooks.slack.com/services/x")
        );
        let e = Event {
            kind: NotifyEvent::RunFailed,
            source: Some("triage".into()),
            subject: String::new(),
            text: String::new(),
            link: None,
            meta: Value::Null,
        };
        assert!(r.matches(&e));
        assert!(!r.matches(&Event {
            source: Some("other".into()),
            ..e.clone()
        }));
        assert!(!r.matches(&Event {
            kind: NotifyEvent::Alert,
            ..e
        }));
        assert!(rule("name: x\non: []\npush: true\n").is_err());
        assert!(rule("name: x\non: [alert]\n").is_err());
        assert!(rule("name: bad name\non: [alert]\npush: true\n").is_err());
        assert!(rule("name: x\non: [alert]\nemail: [nope]\n").is_err());
    }
}
