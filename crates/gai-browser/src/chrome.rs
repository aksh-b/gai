//! Headless Chrome over the DevTools protocol.
//!
//! One Chrome process is started on first use and shared. Each conversation
//! (a session, or a run outside one) gets its own browser context with one
//! page, closed after `idle_secs` unused; Chrome itself exits when no context
//! is left. Every request a page makes is paused, checked against the
//! [`DomainRules`], and then continued or failed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use chromiumoxide::cdp::browser_protocol::browser::BrowserContextId;
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EventRequestPaused, FailRequestParams,
};
use chromiumoxide::cdp::browser_protocol::network::ErrorReason;
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::cdp::browser_protocol::target::{
    CreateBrowserContextParams, CreateTargetParams,
};
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::page::ScreenshotParams;
use chromiumoxide::{Browser, BrowserConfig, Element, Page};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::Value;
use tokio::task::JoinHandle;
use url::Url;

use crate::{BrowserSettings, DomainRules};

const PAGE_JS: &str = include_str!("page.js");

#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error("{0}")]
    Unavailable(String),
    /// A mistake the agent can fix (bad URL, missing element, blocked site).
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Failed(String),
}

type Result<T> = std::result::Result<T, BrowserError>;

fn failed(e: impl std::fmt::Display) -> BrowserError {
    BrowserError::Failed(e.to_string())
}

/// The shared Chrome and the open browser contexts.
pub struct Chrome {
    settings: BrowserSettings,
    executable: PathBuf,
    rules: Arc<DomainRules>,
    state: tokio::sync::Mutex<State>,
    me: Weak<Chrome>,
}

#[derive(Default)]
struct State {
    running: Option<Running>,
    sessions: HashMap<String, Arc<Session>>,
    reaper: Option<JoinHandle<()>>,
}

struct Running {
    browser: Browser,
    handler: JoinHandle<()>,
}

/// One conversation's browser context and its page.
pub struct Session {
    context: BrowserContextId,
    page: Page,
    rules: Arc<DomainRules>,
    timeout: Duration,
    last_used: Mutex<Instant>,
    /// One action at a time on the page.
    busy: tokio::sync::Mutex<()>,
    /// Why the most recent requests were refused, newest last.
    blocked: Arc<Mutex<Vec<String>>>,
    interceptor: JoinHandle<()>,
}

/// What `browser_read` and `browser_navigate` show.
#[derive(Debug, Deserialize)]
pub struct Snapshot {
    pub title: String,
    pub url: String,
    pub text: String,
}

/// What `browser_extract` returns.
#[derive(Debug, Deserialize, serde::Serialize)]
pub struct Extracted {
    pub total: usize,
    pub items: Vec<serde_json::Map<String, Value>>,
}

/// An element by the number `browser_read` gave it, or a CSS selector.
#[derive(Clone, Debug)]
pub enum Target {
    Ref(u32),
    Selector(String),
}

impl Target {
    fn selector(&self) -> String {
        match self {
            Target::Ref(n) => format!("[data-gai-ref=\"{n}\"]"),
            Target::Selector(s) => s.clone(),
        }
    }

    fn describe(&self) -> String {
        match self {
            Target::Ref(n) => format!("[{n}]"),
            Target::Selector(s) => format!("{s:?}"),
        }
    }
}

impl Chrome {
    /// A launcher for the configured Chrome, or `None` when the browser is
    /// turned off or no Chrome is installed.
    pub fn from_settings(settings: &BrowserSettings) -> Option<Arc<Chrome>> {
        if !settings.enabled {
            return None;
        }
        let Some(executable) = settings.find_executable() else {
            tracing::info!(
                "no Chrome or Chromium found: the browser tools are off (set browser.executable)"
            );
            return None;
        };
        Some(Self::new(settings.clone(), executable))
    }

    pub fn new(settings: BrowserSettings, executable: PathBuf) -> Arc<Chrome> {
        let rules = Arc::new(settings.rules());
        Arc::new_cyclic(|me| Chrome {
            settings,
            executable,
            rules,
            state: tokio::sync::Mutex::default(),
            me: me.clone(),
        })
    }

    pub fn settings(&self) -> &BrowserSettings {
        &self.settings
    }

    pub fn rules(&self) -> &DomainRules {
        &self.rules
    }

    /// The browser context for `key`, opening Chrome and the context when
    /// needed.
    pub async fn session(&self, key: &str) -> Result<Arc<Session>> {
        let mut st = self.state.lock().await;
        if let Some(s) = st.sessions.get(key)
            && st
                .running
                .as_ref()
                .is_some_and(|r| !r.handler.is_finished())
        {
            s.touch();
            return Ok(s.clone());
        }
        if st.running.as_ref().is_none_or(|r| r.handler.is_finished()) {
            // Chrome is gone (or never started): its contexts went with it.
            for (_, s) in st.sessions.drain() {
                s.interceptor.abort();
            }
            st.running = Some(self.launch().await?);
        }
        while st.sessions.len() >= self.settings.max_contexts.max(1) {
            let oldest = st
                .sessions
                .iter()
                .min_by_key(|(_, s)| *s.last_used.lock().unwrap())
                .map(|(k, _)| k.clone())
                .expect("sessions is not empty");
            if let Some(s) = st.sessions.remove(&oldest) {
                close(st.running.as_ref(), &s).await;
            }
        }
        let browser = &st.running.as_ref().expect("launched above").browser;
        let session = Arc::new(self.open(browser).await?);
        st.sessions.insert(key.to_owned(), session.clone());
        if st.reaper.as_ref().is_none_or(|r| r.is_finished()) {
            st.reaper = Some(tokio::spawn(reap(self.me.clone())));
        }
        Ok(session)
    }

    /// Close `key`'s context, if open.
    pub async fn close(&self, key: &str) {
        let mut st = self.state.lock().await;
        if let Some(s) = st.sessions.remove(key) {
            close(st.running.as_ref(), &s).await;
        }
    }

    /// Close every context and Chrome itself.
    pub async fn shutdown(&self) {
        let mut st = self.state.lock().await;
        for (_, s) in st.sessions.drain() {
            s.interceptor.abort();
        }
        if let Some(mut r) = st.running.take() {
            let _ = r.browser.close().await;
            let _ = r.browser.wait().await;
            r.handler.abort();
        }
    }

    async fn launch(&self) -> Result<Running> {
        let s = &self.settings;
        let mut cfg = BrowserConfig::builder()
            .chrome_executable(&self.executable)
            .window_size(s.width, s.height)
            .viewport(Viewport {
                width: s.width,
                height: s.height,
                ..Default::default()
            })
            .user_data_dir(profile_dir())
            .request_timeout(Duration::from_secs(s.timeout_secs.max(1)))
            .launch_timeout(Duration::from_secs(30))
            // Chrome then pauses every request for us to check; see `open`.
            .enable_request_intercept()
            .args(s.args.iter().map(String::as_str));
        if !s.headless {
            cfg = cfg.with_head();
        }
        if running_as_root() {
            // Chrome's own sandbox refuses to run as root (as in most
            // containers).
            cfg = cfg.no_sandbox();
        }
        let cfg = cfg.build().map_err(BrowserError::Unavailable)?;
        let (browser, mut handler) = Browser::launch(cfg).await.map_err(|e| {
            BrowserError::Unavailable(format!(
                "could not start {}: {e}",
                self.executable.display()
            ))
        })?;
        let handler = tokio::spawn(async move {
            while let Some(r) = handler.next().await {
                if let Err(e) = r {
                    tracing::debug!(error = %e, "browser event");
                }
            }
        });
        tracing::info!(executable = %self.executable.display(), "browser started");
        Ok(Running { browser, handler })
    }

    async fn open(&self, browser: &Browser) -> Result<Session> {
        let context = browser
            .create_browser_context(CreateBrowserContextParams::default())
            .await
            .map_err(failed)?;
        let target = CreateTargetParams::builder()
            .url("about:blank")
            .browser_context_id(context.clone())
            .build()
            .map_err(BrowserError::Failed)?;
        let page = browser.new_page(target).await.map_err(failed)?;
        let blocked = Arc::new(Mutex::new(Vec::new()));
        let mut paused = page
            .event_listener::<EventRequestPaused>()
            .await
            .map_err(failed)?;
        let interceptor = {
            let (page, rules, blocked) = (page.clone(), self.rules.clone(), blocked.clone());
            tokio::spawn(async move {
                while let Some(ev) = paused.next().await {
                    let verdict = match Url::parse(&ev.request.url) {
                        Ok(u) => rules.check(&u).await,
                        Err(e) => Err(format!("{}: {e}", ev.request.url)),
                    };
                    let sent = match verdict {
                        Ok(()) => page
                            .execute(ContinueRequestParams::new(ev.request_id.clone()))
                            .await
                            .map(|_| ()),
                        Err(why) => {
                            tracing::debug!(url = %ev.request.url, %why, "browser request refused");
                            {
                                let mut b = blocked.lock().unwrap();
                                b.push(why);
                                let n = b.len();
                                if n > 20 {
                                    b.drain(..n - 20);
                                }
                            }
                            page.execute(FailRequestParams::new(
                                ev.request_id.clone(),
                                ErrorReason::BlockedByClient,
                            ))
                            .await
                            .map(|_| ())
                        }
                    };
                    if let Err(e) = sent {
                        tracing::debug!(error = %e, "could not answer a paused request");
                    }
                }
            })
        };
        // New windows would escape the one page we watch.
        page.evaluate_on_new_document("window.open = () => null;")
            .await
            .map_err(failed)?;
        Ok(Session {
            context,
            page,
            rules: self.rules.clone(),
            timeout: Duration::from_secs(self.settings.timeout_secs.max(1)),
            last_used: Mutex::new(Instant::now()),
            busy: tokio::sync::Mutex::new(()),
            blocked,
            interceptor,
        })
    }
}

async fn close(running: Option<&Running>, s: &Session) {
    s.interceptor.abort();
    if let Some(r) = running {
        let _ = r.browser.dispose_browser_context(s.context.clone()).await;
    }
}

/// Close contexts left unused, and Chrome once none is left.
async fn reap(chrome: Weak<Chrome>) {
    loop {
        tokio::time::sleep(Duration::from_secs(15)).await;
        let Some(c) = chrome.upgrade() else { return };
        let idle = Duration::from_secs(c.settings.idle_secs.max(1));
        let mut st = c.state.lock().await;
        let stale: Vec<String> = st
            .sessions
            .iter()
            .filter(|(_, s)| {
                s.last_used.lock().unwrap().elapsed() > idle && s.busy.try_lock().is_ok()
            })
            .map(|(k, _)| k.clone())
            .collect();
        for k in stale {
            if let Some(s) = st.sessions.remove(&k) {
                close(st.running.as_ref(), &s).await;
            }
        }
        if st.sessions.is_empty() {
            if let Some(mut r) = st.running.take() {
                let _ = r.browser.close().await;
                let _ = r.browser.wait().await;
                r.handler.abort();
                tracing::info!("browser closed (idle)");
            }
            st.reaper = None;
            return;
        }
    }
}

/// A fresh profile directory for each Chrome started: two Chromes can't
/// share one.
fn profile_dir() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("gai-browser-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn running_as_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .map(|l| l.split_whitespace().nth(1) == Some("0"))
        })
        .unwrap_or(false)
}

impl Session {
    fn touch(&self) {
        *self.last_used.lock().unwrap() = Instant::now();
    }

    /// Hold the page for one action.
    pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        let g = self.busy.lock().await;
        self.touch();
        g
    }

    fn take_blocked(&self) -> Vec<String> {
        std::mem::take(&mut *self.blocked.lock().unwrap())
    }

    pub async fn url(&self) -> String {
        self.page.url().await.ok().flatten().unwrap_or_default()
    }

    pub async fn title(&self) -> String {
        self.page
            .get_title()
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    /// Load `url`.
    pub async fn navigate(&self, url: &str) -> Result<()> {
        let parsed = Url::parse(url.trim())
            .map_err(|e| BrowserError::Invalid(format!("{url:?} is not a URL: {e}")))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(BrowserError::Invalid(format!(
                "{url}: only http and https URLs can be opened"
            )));
        }
        self.rules
            .check(&parsed)
            .await
            .map_err(BrowserError::Invalid)?;
        self.take_blocked();
        let r = tokio::time::timeout(self.timeout, self.page.goto(parsed.as_str())).await;
        match r {
            Err(_) => Err(BrowserError::Failed(format!(
                "{url} did not finish loading in {}s",
                self.timeout.as_secs()
            ))),
            Ok(Err(e)) => {
                let blocked = self.take_blocked();
                match blocked.last() {
                    Some(why) => Err(BrowserError::Invalid(format!("blocked: {why}"))),
                    None => Err(BrowserError::Failed(format!("could not open {url}: {e}"))),
                }
            }
            Ok(Ok(_)) => Ok(()),
        }
    }

    /// The page as an outline with numbered elements.
    pub async fn outline(&self, max_chars: usize) -> Result<Snapshot> {
        self.eval(&format!("outline({max_chars})")).await
    }

    /// The readable text of the page.
    pub async fn text(&self, max_chars: usize) -> Result<Snapshot> {
        self.eval(&format!("text({max_chars})")).await
    }

    pub async fn extract(
        &self,
        selector: &str,
        attributes: &[String],
        limit: usize,
    ) -> Result<Extracted> {
        let args = serde_json::to_string(&(selector, attributes, limit)).expect("serializable");
        self.eval(&format!("extract({args})"))
            .await
            .map_err(|e| match e {
                BrowserError::Failed(m)
                    if m.contains("SyntaxError") || m.contains("not a valid selector") =>
                {
                    BrowserError::Invalid(format!("{selector:?} is not a valid CSS selector"))
                }
                e => e,
            })
    }

    async fn eval<T: serde::de::DeserializeOwned>(&self, call: &str) -> Result<T> {
        let expr = format!("(() => {{\n{PAGE_JS}\nreturn {call};\n}})()");
        let r = tokio::time::timeout(self.timeout, self.page.evaluate(expr))
            .await
            .map_err(|_| BrowserError::Failed("the page did not answer in time".into()))?
            .map_err(failed)?;
        r.into_value().map_err(failed)
    }

    async fn element(&self, target: &Target) -> Result<Element> {
        self.page
            .find_element(target.selector())
            .await
            .map_err(|_| {
                BrowserError::Invalid(match target {
                    Target::Ref(n) => format!(
                        "no element [{n}] on the page; call browser_read again (the page may have changed)"
                    ),
                    Target::Selector(s) => format!("nothing on the page matches {s:?}"),
                })
            })
    }

    async fn js_bool(&self, el: &Element, function: &str) -> Result<bool> {
        let r = el.call_js_fn(function, false).await.map_err(failed)?;
        Ok(r.result.value.and_then(|v| v.as_bool()).unwrap_or(false))
    }

    /// Click an element. Buttons that submit a form are refused: those go
    /// through [`submit`](Self::submit), which can need approval.
    pub async fn click(&self, target: &Target) -> Result<()> {
        let el = self.element(target).await?;
        if self.js_bool(&el, SUBMITS_JS).await? {
            return Err(BrowserError::Invalid(format!(
                "{} submits a form; use browser_submit for that",
                target.describe()
            )));
        }
        self.take_blocked();
        el.click().await.map_err(failed)?;
        self.settle().await;
        Ok(())
    }

    /// Type into a field (replacing what is there unless `append`), or
    /// choose an option of a `<select>` by its text or value.
    pub async fn type_text(&self, target: &Target, text: &str, append: bool) -> Result<String> {
        let el = self.element(target).await?;
        let kind = el
            .call_js_fn(
                "function() { return this.tagName === 'SELECT' ? 'select' : ('value' in this || this.isContentEditable) ? 'field' : 'other'; }",
                false,
            )
            .await
            .map_err(failed)?
            .result
            .value
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        match kind.as_str() {
            "select" => {
                let f = format!(
                    "function() {{ const want = {}.toLowerCase(); const o = [...this.options].find(o => o.text.trim().toLowerCase() === want || o.value.toLowerCase() === want) || [...this.options].find(o => o.text.toLowerCase().includes(want)); if (!o) return null; this.value = o.value; this.dispatchEvent(new Event('input', {{bubbles: true}})); this.dispatchEvent(new Event('change', {{bubbles: true}})); return o.text.trim(); }}",
                    serde_json::to_string(text).expect("string")
                );
                let chosen = el.call_js_fn(f, false).await.map_err(failed)?.result.value;
                match chosen.as_ref().and_then(|v| v.as_str()) {
                    Some(t) => Ok(format!("Chose {t:?} in {}.", target.describe())),
                    None => Err(BrowserError::Invalid(format!(
                        "{} has no option {text:?}",
                        target.describe()
                    ))),
                }
            }
            "field" => {
                el.scroll_into_view().await.map_err(failed)?;
                el.call_js_fn(
                    if append {
                        "function() { this.focus(); }"
                    } else {
                        "function() { this.focus(); if (this.isContentEditable) this.textContent = ''; else { this.value = ''; this.dispatchEvent(new Event('input', {bubbles: true})); } }"
                    },
                    false,
                )
                .await
                .map_err(failed)?;
                el.type_str(text).await.map_err(failed)?;
                Ok(format!("Typed into {}.", target.describe()))
            }
            _ => Err(BrowserError::Invalid(format!(
                "{} is not a text field or a select",
                target.describe()
            ))),
        }
    }

    /// Submit a form: click its submit button, or submit the form a field
    /// belongs to (pressing Enter when there is no form).
    pub async fn submit(&self, target: &Target) -> Result<()> {
        let el = self.element(target).await?;
        self.take_blocked();
        if self.js_bool(&el, SUBMITS_JS).await? {
            el.click().await.map_err(failed)?;
        } else if self
            .js_bool(
                &el,
                "function() { const f = this.form || (this.tagName === 'FORM' ? this : this.closest('form')); if (!f) return false; f.requestSubmit ? f.requestSubmit() : f.submit(); return true; }",
            )
            .await?
        {
        } else {
            el.focus().await.map_err(failed)?;
            el.press_key("Enter").await.map_err(failed)?;
        }
        self.settle().await;
        if let Some(why) = self.take_blocked().last() {
            return Err(BrowserError::Invalid(format!("blocked: {why}")));
        }
        Ok(())
    }

    pub async fn screenshot(&self, full_page: bool) -> Result<Vec<u8>> {
        let params = ScreenshotParams::builder()
            .format(CaptureScreenshotFormat::Png)
            .full_page(full_page)
            .build();
        tokio::time::timeout(self.timeout, self.page.screenshot(params))
            .await
            .map_err(|_| BrowserError::Failed("the screenshot took too long".into()))?
            .map_err(failed)
    }

    /// Give an action time to start a navigation, then wait for the page
    /// to finish loading (at most 10 s).
    async fn settle(&self) {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let deadline = Instant::now() + self.timeout.min(Duration::from_secs(10));
        while Instant::now() < deadline {
            let state = self
                .page
                .evaluate("document.readyState")
                .await
                .ok()
                .and_then(|r| r.into_value::<String>().ok());
            if state.as_deref() == Some("complete") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    /// Requests refused since the last action, for the agent to see.
    pub fn blocked_note(&self) -> Option<String> {
        let b = self.take_blocked();
        (!b.is_empty()).then(|| {
            let mut uniq: Vec<&String> = Vec::new();
            for w in &b {
                if !uniq.contains(&w) {
                    uniq.push(w);
                }
            }
            format!(
                "{} request(s) were blocked: {}",
                b.len(),
                uniq.iter()
                    .take(3)
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        })
    }
}

const SUBMITS_JS: &str = "function() { const t = (this.getAttribute('type') || '').toLowerCase(); if (this.tagName === 'BUTTON') return !!this.form && (t === '' || t === 'submit'); if (this.tagName === 'INPUT') return !!this.form && (t === 'submit' || t === 'image'); return false; }";
