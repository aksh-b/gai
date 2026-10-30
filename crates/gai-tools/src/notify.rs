//! Sending things to people: email (SMTP, or a folder of `.eml` files for
//! local use and tests) and webhooks (JSON, optionally signed). Reports and
//! alerts use it.

use std::time::Duration;

use base64::Engine;
use hmac::{Hmac, Mac};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;

/// ```toml
/// [notify]
/// smtp_url = "smtps://gai@example.com@smtp.example.com:465"   # or smtp://host:587 (STARTTLS), file:///var/mail/gai
/// password_env = "GAI_SMTP_PASSWORD"
/// from = "gai <gai@example.com>"
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct NotifySettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smtp_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_env: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The dashboard's address, for links in notifications
    /// (`https://gai.example.com`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard_url: Option<String>,
    /// Contact for web push services (`mailto:` or `https:`); default from
    /// `from`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push_subject: Option<String>,
    /// Environment variable with the web push (VAPID) private key, base64url
    /// PKCS#8. Without it a key is made once and kept in the database.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push_key_env: Option<String>,
}

impl NotifySettings {
    /// The contact push services see.
    pub fn push_subject(&self) -> String {
        if let Some(s) = &self.push_subject {
            return s.clone();
        }
        let addr = self
            .from
            .as_deref()
            .and_then(|f| f.parse::<lettre::message::Mailbox>().ok())
            .map(|m| m.email.to_string());
        format!("mailto:{}", addr.as_deref().unwrap_or("gai@localhost"))
    }
}

/// Where to send something.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Delivery {
    /// Email addresses.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub email: Vec<String>,
    /// A URL that gets the message as JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook: Option<String>,
    /// Environment variable with a secret: the webhook is signed with it
    /// (`X-Gai-Signature: sha256=<hex HMAC of the body>`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook_secret_env: Option<String>,
    /// A Slack incoming-webhook URL (`https://hooks.slack.com/services/...`).
    /// It is a secret: prefer `slack_env`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slack: Option<String>,
    /// Environment variable holding the Slack incoming-webhook URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slack_env: Option<String>,
    /// The email subject (default: from the message).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

impl Delivery {
    pub fn is_empty(&self) -> bool {
        self.email.is_empty()
            && self.webhook.is_none()
            && self.slack.is_none()
            && self.slack_env.is_none()
    }

    pub fn validate(&self) -> Result<(), String> {
        for e in &self.email {
            if e.parse::<lettre::Address>().is_err() {
                return Err(format!("{e:?} isn't an email address"));
            }
        }
        if let Some(w) = &self.webhook {
            let u = reqwest::Url::parse(w).map_err(|e| format!("webhook: {e}"))?;
            if !matches!(u.scheme(), "http" | "https") {
                return Err("webhook must be http(s)".into());
            }
        }
        if let Some(s) = &self.slack {
            let u = reqwest::Url::parse(s).map_err(|e| format!("slack: {e}"))?;
            if u.scheme() != "https" {
                return Err("slack must be an https webhook URL".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub name: String,
    pub media_type: String,
    pub data: Vec<u8>,
}

/// What to send.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notice {
    pub subject: String,
    /// Plain text or Markdown.
    pub text: String,
    pub attachments: Vec<Attachment>,
    /// Extra fields for webhooks (`kind`, `run_id`, ...).
    pub meta: Value,
}

#[derive(Clone)]
pub struct Notifier {
    settings: NotifySettings,
    http: reqwest::Client,
    allow_private: bool,
}

impl Notifier {
    pub fn new(settings: NotifySettings, allow_private: bool) -> Self {
        Self {
            settings,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("http client"),
            allow_private,
        }
    }

    pub fn settings(&self) -> &NotifySettings {
        &self.settings
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Send `notice` everywhere `to` says; what was sent, or the first
    /// failure (the others are still tried).
    pub async fn send(&self, to: &Delivery, notice: &Notice) -> Result<Vec<String>, String> {
        let mut sent = Vec::new();
        let mut errors = Vec::new();
        if !to.email.is_empty() {
            match self.email(to, notice).await {
                Ok(()) => sent.push(format!("email to {}", to.email.join(", "))),
                Err(e) => errors.push(format!("email: {e}")),
            }
        }
        if let Some(url) = &to.webhook {
            match self
                .webhook(url, to.webhook_secret_env.as_deref(), notice)
                .await
            {
                Ok(()) => sent.push(format!("webhook {url}")),
                Err(e) => errors.push(format!("webhook: {e}")),
            }
        }
        let slack = match (&to.slack, &to.slack_env) {
            (Some(url), _) => Some(Ok(url.clone())),
            (None, Some(var)) => {
                Some(std::env::var(var).map_err(|_| format!("set {var} (the Slack webhook URL)")))
            }
            (None, None) => None,
        };
        match slack {
            Some(Ok(url)) => match self.slack(&url, notice).await {
                Ok(()) => sent.push("slack".to_owned()),
                Err(e) => errors.push(format!("slack: {e}")),
            },
            Some(Err(e)) => errors.push(format!("slack: {e}")),
            None => {}
        }
        match errors.first() {
            Some(e) => Err(e.clone()),
            None => Ok(sent),
        }
    }

    async fn email(&self, to: &Delivery, notice: &Notice) -> Result<(), String> {
        use lettre::message::{
            Attachment as Part, Mailbox, MultiPart, SinglePart, header::ContentType,
        };
        use lettre::{AsyncTransport, Tokio1Executor};
        let url = self
            .settings
            .smtp_url
            .clone()
            .ok_or("set [notify] smtp_url to send email")?;
        let from: Mailbox = self
            .settings
            .from
            .as_deref()
            .unwrap_or("gai <gai@localhost>")
            .parse()
            .map_err(|e| format!("[notify] from: {e}"))?;
        let mut b = lettre::Message::builder()
            .from(from)
            .subject(to.subject.clone().unwrap_or_else(|| notice.subject.clone()));
        for addr in &to.email {
            b = b.to(addr.parse().map_err(|e| format!("{addr}: {e}"))?);
        }
        let mut body = MultiPart::mixed().singlepart(SinglePart::plain(notice.text.clone()));
        for a in &notice.attachments {
            let ct = ContentType::parse(&a.media_type)
                .unwrap_or(ContentType::parse("application/octet-stream").expect("valid"));
            body = body.singlepart(Part::new(a.name.clone()).body(a.data.clone(), ct));
        }
        let msg = b.multipart(body).map_err(|e| e.to_string())?;
        if let Some(dir) = url.strip_prefix("file://") {
            std::fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))?;
            let t = lettre::AsyncFileTransport::<Tokio1Executor>::new(dir);
            t.send(msg).await.map_err(|e| e.to_string())?;
            return Ok(());
        }
        let mut u = reqwest::Url::parse(&url).map_err(|e| format!("[notify] smtp_url: {e}"))?;
        if let Some(var) = &self.settings.password_env {
            let pw = std::env::var(var).map_err(|_| format!("set {var} (the SMTP password)"))?;
            u.set_password(Some(&pw))
                .map_err(|_| "[notify] smtp_url needs a user for the password".to_string())?;
        }
        let t = lettre::AsyncSmtpTransport::<Tokio1Executor>::from_url(u.as_str())
            .map_err(|e| format!("[notify] smtp_url: {e}"))?
            .timeout(Some(Duration::from_secs(30)))
            .build();
        t.send(msg).await.map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Slack's incoming webhooks take `{"text": ...}` in their own markup.
    async fn slack(&self, url: &str, notice: &Notice) -> Result<(), String> {
        let mut text = format!("*{}*", notice.subject.replace('*', ""));
        if !notice.text.is_empty() {
            text.push('\n');
            text.push_str(&slack_text(&notice.text));
        }
        if let Some(link) = notice.meta["url"].as_str() {
            text.push_str(&format!("\n<{link}|Open in gai>"));
        }
        let r = self
            .http
            .post(url)
            .json(&json!({ "text": text }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(format!("Slack answered {}", r.status()));
        }
        Ok(())
    }

    async fn webhook(
        &self,
        url: &str,
        secret_env: Option<&str>,
        notice: &Notice,
    ) -> Result<(), String> {
        let u = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
        if !self.allow_private
            && crate::builtin::is_private_host(
                u.host_str().unwrap_or_default(),
                u.port_or_known_default().unwrap_or(80),
            )
        {
            return Err(format!("{url} is a private network address"));
        }
        let mut payload = json!({
            "subject": notice.subject,
            "text": notice.text,
            "attachments": notice.attachments.iter().map(|a| json!({
                "name": a.name,
                "media_type": a.media_type,
                "data_base64": base64::engine::general_purpose::STANDARD.encode(&a.data),
            })).collect::<Vec<_>>(),
        });
        if let Value::Object(m) = &notice.meta {
            for (k, v) in m {
                payload[k] = v.clone();
            }
        }
        let body = serde_json::to_vec(&payload).expect("json");
        let mut rb = self.http.post(u).header("content-type", "application/json");
        if let Some(var) = secret_env {
            let secret =
                std::env::var(var).map_err(|_| format!("set {var} (the webhook secret)"))?;
            let mut mac =
                Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("any key length");
            mac.update(&body);
            let sig: String = mac
                .finalize()
                .into_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            rb = rb.header("X-Gai-Signature", format!("sha256={sig}"));
        }
        let r = rb.body(body).send().await.map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(format!("{url} answered {}", r.status()));
        }
        Ok(())
    }
}

/// Markdown to Slack's mrkdwn: `**bold**` to `*bold*`, `[a](b)` to `<b|a>`,
/// and `&`, `<`, `>` escaped.
fn slack_text(md: &str) -> String {
    let escaped = md
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let bold = regex::Regex::new(r"\*\*([^*]+)\*\*").expect("valid");
    let link = regex::Regex::new(r"\[([^\]]+)\]\((https?://[^)\s]+)\)").expect("valid");
    let s = bold.replace_all(&escaped, "*$1*");
    link.replace_all(&s, "<$2|$1>").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_becomes_slack_markup() {
        assert_eq!(
            slack_text("**Run failed** in [triage](https://gai.example.com/#/runs/1) <now>"),
            "*Run failed* in <https://gai.example.com/#/runs/1|triage> &lt;now&gt;"
        );
    }

    #[tokio::test]
    async fn slack_gets_text() {
        use wiremock::matchers::{body_partial_json, method};
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(method("POST"))
            .and(body_partial_json(
                json!({"text": "*Approval needed*\nshell wants to run"}),
            ))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let n = Notifier::new(NotifySettings::default(), true);
        let to = Delivery {
            slack: Some(server.uri()),
            ..Default::default()
        };
        let notice = Notice {
            subject: "Approval needed".into(),
            text: "shell wants to run".into(),
            ..Default::default()
        };
        assert_eq!(n.send(&to, &notice).await.unwrap(), ["slack"]);
    }

    #[tokio::test]
    async fn email_to_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let n = Notifier::new(
            NotifySettings {
                smtp_url: Some(format!("file://{}", dir.path().display())),
                from: Some("gai <gai@example.com>".into()),
                ..Default::default()
            },
            true,
        );
        let to = Delivery {
            email: vec!["ana@example.com".into()],
            ..Default::default()
        };
        let sent = n
            .send(
                &to,
                &Notice {
                    subject: "Weekly signups".into(),
                    text: "Signups fell 12%.".into(),
                    attachments: vec![Attachment {
                        name: "report.pdf".into(),
                        media_type: "application/pdf".into(),
                        data: b"%PDF-1.4 tiny".to_vec(),
                    }],
                    meta: json!({}),
                },
            )
            .await
            .unwrap();
        assert_eq!(sent, ["email to ana@example.com"]);
        let mail = std::fs::read_dir(dir.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let text = std::fs::read_to_string(mail).unwrap();
        assert!(
            text.contains("Subject: Weekly signups")
                && text.contains("report.pdf")
                && text.contains("ana@example.com"),
            "{text}"
        );
        assert!(
            Delivery {
                email: vec!["nope".into()],
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        let no_smtp = Notifier::new(NotifySettings::default(), true);
        assert!(
            no_smtp
                .send(&to, &Notice::default())
                .await
                .unwrap_err()
                .contains("smtp_url")
        );
    }
}
