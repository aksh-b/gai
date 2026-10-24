//! The browser tools against a local fixture site. Skipped (with a note)
//! when no Chrome or Chromium is installed; the dev image has Chromium.

#![cfg(feature = "chrome")]

use std::sync::Arc;

use axum::Router;
use axum::extract::Query;
use axum::response::Html;
use axum::routing::get;
use gai_browser::BrowserSettings;
use gai_browser::chrome::Chrome;
use gai_core::ContentBlock;
use gai_tools::{SharedTool, ToolCtx, ToolError};
use serde_json::{Value, json};

const HOME: &str = r#"<!doctype html><html><head><title>Fixture home</title></head><body>
<h1>Welcome</h1>
<p>Some <b>intro</b> text.</p>
<p id="late"></p>
<script>document.getElementById('late').textContent = 'Rendered by JavaScript';</script>
<a href="/two">Second page</a>
<form action="/search" method="get">
  <label for="q">Query</label> <input id="q" name="q" value="old">
  <select name="color"><option value="r">Red</option><option value="g">Green</option></select>
  <button type="submit">Search</button>
</form>
<button id="plain" type="button" onclick="this.textContent='clicked!'">Press me</button>
<ul class="items"><li><a href="/a">Alpha</a></li><li><a href="/b">Beta</a></li></ul>
<img src="http://localhost:1/tracker.png" alt="">
</body></html>"#;

#[derive(serde::Deserialize)]
struct Search {
    q: String,
    color: String,
}

async fn site() -> String {
    let app = Router::new()
        .route("/", get(|| async { Html(HOME) }))
        .route(
            "/two",
            get(|| async { Html("<title>Two</title><h2>Page two</h2><p>second</p>") }),
        )
        .route(
            "/search",
            get(|Query(s): Query<Search>| async move {
                Html(format!(
                    "<title>Results</title><p>You searched for {} in {}</p>",
                    s.q, s.color
                ))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

struct Kit {
    tools: Vec<SharedTool>,
    ctx: ToolCtx,
    chrome: Arc<Chrome>,
}

impl Kit {
    async fn call(&self, name: &str, input: Value) -> Result<String, ToolError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.spec().name == name)
            .unwrap_or_else(|| panic!("no tool {name}"));
        let out = tool.call(&self.ctx, input).await?;
        Ok(out
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn kit(settings: BrowserSettings) -> Option<Kit> {
    let Some(chrome) = Chrome::from_settings(&settings) else {
        eprintln!("skipped: no Chrome or Chromium installed");
        return None;
    };
    let dir = std::env::temp_dir();
    Some(Kit {
        tools: gai_browser::tools(chrome.clone(), None),
        ctx: ToolCtx::for_workspace(dir),
        chrome,
    })
}

fn local() -> BrowserSettings {
    BrowserSettings {
        allow_private_network: true,
        // The fixture is 127.0.0.1; the tracker image is localhost.
        deny_domains: vec!["localhost".into()],
        ..Default::default()
    }
}

#[tokio::test]
async fn reads_clicks_types_and_submits() {
    let Some(k) = kit(local()) else { return };
    let base = site().await;

    let page = k
        .call("browser_navigate", json!({"url": format!("{base}/")}))
        .await
        .unwrap();
    assert!(page.starts_with("Fixture home"), "{page}");
    assert!(page.contains("# Welcome"), "{page}");
    assert!(page.contains("Rendered by JavaScript"), "{page}");
    assert!(page.contains("link \"Second page\" -> /two"), "{page}");
    assert!(page.contains("textbox \"Query\" = \"old\""), "{page}");
    assert!(
        page.contains("button \"Search\" (submits a form)"),
        "{page}"
    );
    // The tracker on a denied host was refused.
    assert!(page.contains("blocked"), "{page}");
    let num = |label: &str| -> u64 {
        let line = page
            .lines()
            .find(|l| l.starts_with('[') && l.contains(label))
            .unwrap_or_else(|| {
                panic!(
                    "no {label:?} in
{page}"
                )
            });
        line[1..line.find(']').unwrap()].parse().unwrap()
    };

    // A plain button clicks; a submit button is refused.
    let plain = num("Press me");
    k.call("browser_click", json!({"ref": plain}))
        .await
        .unwrap();
    let text = k
        .call("browser_read", json!({"mode": "text"}))
        .await
        .unwrap();
    assert!(text.contains("clicked!"), "{text}");
    let err = k
        .call("browser_click", json!({"ref": num("Search")}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("browser_submit"), "{err}");

    // Fill the form and submit it through a field.
    k.call("browser_type", json!({"ref": num("Query"), "text": "rust"}))
        .await
        .unwrap();
    let chose = k
        .call(
            "browser_type",
            json!({"selector": "select", "text": "green"}),
        )
        .await
        .unwrap();
    assert!(chose.contains("Green"), "{chose}");
    let done = k
        .call(
            "browser_submit",
            json!({"ref": format!("[{}]", num("Query"))}),
        )
        .await
        .unwrap();
    assert!(done.contains("Results"), "{done}");
    let text = k
        .call("browser_read", json!({"mode": "text"}))
        .await
        .unwrap();
    assert!(text.contains("You searched for rust in g"), "{text}");

    // Links, extraction and screenshots.
    k.call("browser_navigate", json!({"url": format!("{base}/")}))
        .await
        .unwrap();
    let got = k
        .call(
            "browser_extract",
            json!({"selector": "ul.items a", "attributes": ["href"]}),
        )
        .await
        .unwrap();
    let got: Value = serde_json::from_str(&got).unwrap();
    assert_eq!(got["total"], 2);
    assert_eq!(got["items"][1]["text"], "Beta");
    assert_eq!(got["items"][1]["href"], format!("{base}/b"));
    let page = k
        .call("browser_navigate", json!({"url": format!("{base}/")}))
        .await
        .unwrap();
    let second = page
        .lines()
        .find(|l| l.contains("Second page"))
        .map(|l| l[1..l.find(']').unwrap()].to_owned())
        .unwrap();
    let moved = k
        .call("browser_click", json!({"ref": second}))
        .await
        .unwrap();
    assert!(moved.contains("/two"), "{moved}");
    let shot = k
        .tools
        .iter()
        .find(|t| t.spec().name == "browser_screenshot")
        .unwrap();
    let out = shot.call(&k.ctx, json!({})).await.unwrap();
    assert!(
        out.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Image { .. })),
        "{out:?}"
    );

    // Stale numbers say what to do.
    let err = k
        .call("browser_click", json!({"ref": 999}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("browser_read"), "{err}");
    k.chrome.shutdown().await;
}

#[tokio::test]
async fn refuses_sites_outside_the_rules() {
    let Some(k) = kit(BrowserSettings {
        allow_domains: vec!["example.com".into()],
        ..Default::default()
    }) else {
        return;
    };
    let base = site().await;
    // Not on the allow list, and private.
    let err = k
        .call("browser_navigate", json!({"url": format!("{base}/")}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("allow list"), "{err}");
    let err = k
        .call("browser_navigate", json!({"url": "file:///etc/passwd"}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("http"), "{err}");

    // Private addresses need allow_private_network.
    let Some(k2) = kit(BrowserSettings::default()) else {
        return;
    };
    let err = k2
        .call("browser_navigate", json!({"url": format!("{base}/")}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("private"), "{err}");
    k.chrome.shutdown().await;
    k2.chrome.shutdown().await;
}

#[tokio::test]
async fn submitting_needs_approval_by_default() {
    let chrome = Chrome::new(BrowserSettings::default(), "/nonexistent".into());
    let tools = gai_browser::tools(chrome, None);
    for t in &tools {
        let expected = if t.spec().name == "browser_submit" {
            gai_tools::ToolPolicy::Approve
        } else {
            gai_tools::ToolPolicy::Auto
        };
        assert_eq!(t.default_policy(), expected, "{}", t.spec().name);
    }
}
