//! The browser tools.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use gai_core::{ContentBlock, MediaSource, ToolSpec};
use gai_tools::{SharedTool, Tool, ToolCtx, ToolError, ToolOutput, ToolPolicy};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::chrome::{BrowserError, Chrome, Session, Snapshot, Target};

/// Default size of a page outline, in characters.
const OUTLINE_CHARS: usize = 12_000;

/// Where screenshots go, so they are stored once and shown to the user.
#[async_trait]
pub trait ImageSink: Send + Sync {
    /// Keep `data` and return its id.
    async fn save(
        &self,
        ctx: &ToolCtx,
        name: &str,
        media_type: &str,
        data: Vec<u8>,
    ) -> Result<String, String>;
}

/// The browser tools over `chrome`. Screenshots go to `images` when given,
/// and inline otherwise.
pub fn tools(chrome: Arc<Chrome>, images: Option<Arc<dyn ImageSink>>) -> Vec<SharedTool> {
    let b = || Browse {
        chrome: chrome.clone(),
        images: images.clone(),
    };
    vec![
        Arc::new(Navigate(b())),
        Arc::new(Read(b())),
        Arc::new(Click(b())),
        Arc::new(Type(b())),
        Arc::new(Submit(b())),
        Arc::new(Extract(b())),
        Arc::new(Screenshot(b())),
    ]
}

struct Browse {
    chrome: Arc<Chrome>,
    images: Option<Arc<dyn ImageSink>>,
}

impl Browse {
    /// The conversation's browser: chat turns share a page, so the agent
    /// can carry on where it left off.
    async fn session(&self, ctx: &ToolCtx) -> Result<Arc<Session>, ToolError> {
        let key = if ctx.session_id.is_empty() {
            &ctx.run_id
        } else {
            &ctx.session_id
        };
        self.chrome.session(key).await.map_err(err)
    }

    fn max_chars(&self, ctx: &ToolCtx, asked: Option<usize>) -> usize {
        asked
            .unwrap_or(OUTLINE_CHARS)
            .clamp(500, ctx.settings.max_output_chars.max(500))
    }
}

fn err(e: BrowserError) -> ToolError {
    match e {
        BrowserError::Invalid(m) => ToolError::InvalidInput(m),
        e => ToolError::Failed(e.to_string()),
    }
}

fn args<T: serde::de::DeserializeOwned>(input: Value) -> Result<T, ToolError> {
    serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))
}

fn page_text(s: &Snapshot) -> String {
    format!("{}\n{}\n\n{}", s.title, s.url, s.text)
}

/// `ref` or `selector`, as the element tools take them.
#[derive(Deserialize)]
struct ElementArgs {
    #[serde(default, rename = "ref")]
    reference: Option<Value>,
    #[serde(default)]
    selector: Option<String>,
}

impl ElementArgs {
    fn target(&self) -> Result<Target, ToolError> {
        let n = match &self.reference {
            Some(Value::Number(n)) => n.as_u64(),
            // Models write "[3]" or "3" as often as 3.
            Some(Value::String(s)) => s.trim().trim_matches(['[', ']']).parse().ok(),
            _ => None,
        };
        match (n, self.selector.as_deref().map(str::trim)) {
            (Some(n), _) => Ok(Target::Ref(n as u32)),
            (None, Some(s)) if !s.is_empty() => Ok(Target::Selector(s.to_owned())),
            _ => Err(ToolError::InvalidInput(
                "give `ref` (a number from browser_read) or a CSS `selector`".into(),
            )),
        }
    }
}

fn element_props() -> Value {
    json!({
        "ref": {"type": "integer", "description": "The element's number from browser_read"},
        "selector": {"type": "string", "description": "A CSS selector, instead of ref"}
    })
}

/// After an action: where the page is now, and anything blocked.
async fn after(session: &Session, done: &str, before: &str) -> ToolOutput {
    let url = session.url().await;
    let title = session.title().await;
    let mut text = if url != before {
        format!("{done} The page is now {title:?} ({url}). Call browser_read to see it.")
    } else {
        format!("{done} Still on {title:?}. Call browser_read to see what changed.")
    };
    if let Some(note) = session.blocked_note() {
        text.push('\n');
        text.push_str(&note);
    }
    ToolOutput::text(text)
}

struct Navigate(Browse);

#[derive(Deserialize)]
struct NavigateArgs {
    url: String,
    #[serde(default)]
    max_chars: Option<usize>,
}

#[async_trait]
impl Tool for Navigate {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "browser_navigate".into(),
            description: "Open a web page in a real browser (JavaScript runs, so this works for sites http_fetch can't read) and get an outline of it. In the outline, each link, button and field has a number like [3]: pass it as `ref` to browser_click, browser_type or browser_submit.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "An http or https URL"},
                    "max_chars": {"type": "integer", "description": format!("Longest outline to return (default {OUTLINE_CHARS})")}
                },
                "required": ["url"]
            }),
            strict: false,
        }
    }

    fn idempotent(&self) -> bool {
        true
    }

    async fn call(&self, ctx: &ToolCtx, input: Value) -> Result<ToolOutput, ToolError> {
        let a: NavigateArgs = args(input)?;
        let s = self.0.session(ctx).await?;
        let _g = s.lock().await;
        s.navigate(&a.url).await.map_err(err)?;
        let snap = s
            .outline(self.0.max_chars(ctx, a.max_chars))
            .await
            .map_err(err)?;
        let mut text = page_text(&snap);
        if let Some(note) = s.blocked_note() {
            text.push_str(&format!("\n\n({note})"));
        }
        Ok(ToolOutput::text(text))
    }
}

struct Read(Browse);

#[derive(Deserialize)]
struct ReadArgs {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    max_chars: Option<usize>,
}

#[async_trait]
impl Tool for Read {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "browser_read".into(),
            description: "Read the page open in the browser. `outline` (the default) lists headings, text and numbered links, buttons and fields; `text` gives just the readable text of the main content, for reading articles.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "mode": {"type": "string", "enum": ["outline", "text"]},
                    "max_chars": {"type": "integer", "description": format!("Longest reply (default {OUTLINE_CHARS})")}
                }
            }),
            strict: false,
        }
    }

    fn idempotent(&self) -> bool {
        true
    }

    async fn call(&self, ctx: &ToolCtx, input: Value) -> Result<ToolOutput, ToolError> {
        let a: ReadArgs = args(input)?;
        let s = self.0.session(ctx).await?;
        let _g = s.lock().await;
        let max = self.0.max_chars(ctx, a.max_chars);
        let snap = match a.mode.as_deref().unwrap_or("outline") {
            "text" => s.text(max).await,
            "outline" => s.outline(max).await,
            m => {
                return Err(ToolError::InvalidInput(format!(
                    "mode {m:?}: use outline or text"
                )));
            }
        }
        .map_err(err)?;
        if snap.url == "about:blank" {
            return Ok(ToolOutput::text(
                "No page is open yet; use browser_navigate first.",
            ));
        }
        Ok(ToolOutput::text(page_text(&snap)))
    }
}

struct Click(Browse);

#[async_trait]
impl Tool for Click {
    fn spec(&self) -> ToolSpec {
        let mut props = element_props();
        props["ref"]["description"] = "The number of the link or button from browser_read".into();
        ToolSpec {
            name: "browser_click".into(),
            description: "Click a link, button, checkbox or tab on the page. Buttons that submit a form need browser_submit instead.".into(),
            input_schema: json!({"type": "object", "properties": props}),
            strict: false,
        }
    }

    async fn call(&self, ctx: &ToolCtx, input: Value) -> Result<ToolOutput, ToolError> {
        let target = args::<ElementArgs>(input)?.target()?;
        let s = self.0.session(ctx).await?;
        let _g = s.lock().await;
        let before = s.url().await;
        s.click(&target).await.map_err(err)?;
        Ok(after(&s, "Clicked.", &before).await)
    }
}

struct Type(Browse);

#[derive(Deserialize)]
struct TypeArgs {
    #[serde(flatten)]
    element: ElementArgs,
    text: String,
    #[serde(default)]
    append: bool,
}

#[async_trait]
impl Tool for Type {
    fn spec(&self) -> ToolSpec {
        let mut props = element_props();
        props["text"] = json!({"type": "string", "description": "What to type, or the option to choose in a select"});
        props["append"] = json!({"type": "boolean", "description": "Add to the field's text instead of replacing it"});
        ToolSpec {
            name: "browser_type".into(),
            description: "Type into a text field on the page, or choose an option in a dropdown. This does not submit the form; use browser_submit for that.".into(),
            input_schema: json!({"type": "object", "properties": props, "required": ["text"]}),
            strict: false,
        }
    }

    async fn call(&self, ctx: &ToolCtx, input: Value) -> Result<ToolOutput, ToolError> {
        let a: TypeArgs = args(input)?;
        let target = a.element.target()?;
        let s = self.0.session(ctx).await?;
        let _g = s.lock().await;
        let done = s.type_text(&target, &a.text, a.append).await.map_err(err)?;
        Ok(ToolOutput::text(done))
    }
}

struct Submit(Browse);

#[async_trait]
impl Tool for Submit {
    fn spec(&self) -> ToolSpec {
        let mut props = element_props();
        props["ref"]["description"] =
            "The form's submit button, or any field of the form, from browser_read".into();
        ToolSpec {
            name: "browser_submit".into(),
            description: "Submit a form on the page (search boxes, sign-ups, orders, messages): clicks its submit button, or submits the form the given field is in. Fill the fields with browser_type first.".into(),
            input_schema: json!({"type": "object", "properties": props}),
            strict: false,
        }
    }

    /// Submitting can send data or change things on a site.
    fn default_policy(&self) -> ToolPolicy {
        ToolPolicy::Approve
    }

    async fn call(&self, ctx: &ToolCtx, input: Value) -> Result<ToolOutput, ToolError> {
        let target = args::<ElementArgs>(input)?.target()?;
        let s = self.0.session(ctx).await?;
        let _g = s.lock().await;
        let before = s.url().await;
        s.submit(&target).await.map_err(err)?;
        Ok(after(&s, "Submitted.", &before).await)
    }
}

struct Extract(Browse);

#[derive(Deserialize)]
struct ExtractArgs {
    selector: String,
    #[serde(default)]
    attributes: Vec<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for Extract {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "browser_extract".into(),
            description: "Get every element matching a CSS selector as JSON: its text plus the attributes you ask for (href and src come back as full URLs). Use it to collect lists, tables and links.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "selector": {"type": "string", "description": "A CSS selector, e.g. \"table tr\" or \"a.result\""},
                    "attributes": {"type": "array", "items": {"type": "string"}, "description": "Attributes to include, e.g. [\"href\"]"},
                    "limit": {"type": "integer", "description": "Most elements to return (default 50)"}
                },
                "required": ["selector"]
            }),
            strict: false,
        }
    }

    fn idempotent(&self) -> bool {
        true
    }

    async fn call(&self, ctx: &ToolCtx, input: Value) -> Result<ToolOutput, ToolError> {
        let a: ExtractArgs = args(input)?;
        let s = self.0.session(ctx).await?;
        let _g = s.lock().await;
        let found = s
            .extract(
                &a.selector,
                &a.attributes,
                a.limit.unwrap_or(50).clamp(1, 500),
            )
            .await
            .map_err(err)?;
        Ok(ToolOutput::json(&found).truncated(ctx.settings.max_output_chars))
    }
}

struct Screenshot(Browse);

#[derive(Deserialize)]
struct ScreenshotArgs {
    #[serde(default)]
    full_page: bool,
}

#[async_trait]
impl Tool for Screenshot {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "browser_screenshot".into(),
            description: "Take a screenshot of the page open in the browser, to see its layout, images or charts. The user sees it too.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "full_page": {"type": "boolean", "description": "The whole page instead of the visible part"}
                }
            }),
            strict: false,
        }
    }

    fn idempotent(&self) -> bool {
        true
    }

    async fn call(&self, ctx: &ToolCtx, input: Value) -> Result<ToolOutput, ToolError> {
        let a: ScreenshotArgs = args(input)?;
        let s = self.0.session(ctx).await?;
        let _g = s.lock().await;
        let png = s.screenshot(a.full_page).await.map_err(err)?;
        let url = s.url().await;
        let media_type = "image/png".to_owned();
        let source = match &self.0.images {
            Some(sink) => match sink
                .save(ctx, "screenshot.png", &media_type, png.clone())
                .await
            {
                Ok(id) => MediaSource::Blob { id, media_type },
                Err(e) => {
                    tracing::warn!(error = %e, "could not keep a screenshot");
                    inline(&png, media_type)
                }
            },
            None => inline(&png, media_type),
        };
        let mut out = ToolOutput::text(format!("Screenshot of {url}"));
        out.content.push(ContentBlock::Image { source });
        Ok(out)
    }
}

fn inline(png: &[u8], media_type: String) -> MediaSource {
    MediaSource::Base64 {
        media_type,
        data: base64::engine::general_purpose::STANDARD.encode(png),
    }
}
