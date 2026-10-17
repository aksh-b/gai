use std::collections::BTreeMap;
use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

use async_trait::async_trait;
use gai_core::ToolSpec;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Tool, ToolCtx, ToolError, ToolOutput};

/// Fetch a URL. Private and loopback addresses are refused unless
/// `allow_private_network` is set, so an agent cannot be steered into
/// probing the host's network.
pub struct HttpFetch;

#[derive(Deserialize)]
struct Args {
    url: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    body: Option<String>,
}

#[async_trait]
impl Tool for HttpFetch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "http_fetch".into(),
            description: "Make an HTTP request and return the status and response body (HTML is reduced to text). Use for web pages and JSON APIs.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "Absolute http(s) URL"},
                    "method": {"type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"], "default": "GET"},
                    "headers": {"type": "object", "additionalProperties": {"type": "string"}},
                    "body": {"type": "string", "description": "Request body for POST/PUT/PATCH"}
                },
                "required": ["url"]
            }),
            strict: false,
        }
    }

    async fn call(&self, ctx: &ToolCtx, input: Value) -> Result<ToolOutput, ToolError> {
        let a: Args =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let url = reqwest::Url::parse(&a.url)
            .map_err(|e| ToolError::InvalidInput(format!("{}: {e}", a.url)))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ToolError::InvalidInput(
                "only http and https URLs are allowed".into(),
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| ToolError::InvalidInput("URL has no host".into()))?
            .to_owned();
        let port = url.port_or_known_default().unwrap_or(80);
        if !ctx.settings.allow_private_network {
            let private = tokio::task::spawn_blocking(move || is_private_host(&host, port))
                .await
                .map_err(|e| ToolError::Failed(e.to_string()))?;
            if private {
                return Err(ToolError::InvalidInput(format!(
                    "{} resolves to a private or loopback address; enable tools.allow_private_network to allow it",
                    url.host_str().unwrap_or_default()
                )));
            }
        }
        let method = reqwest::Method::from_bytes(
            a.method
                .as_deref()
                .unwrap_or("GET")
                .to_uppercase()
                .as_bytes(),
        )
        .map_err(|_| ToolError::InvalidInput("bad method".into()))?;
        let mut rb = ctx
            .http
            .request(method, url)
            .timeout(Duration::from_secs(ctx.settings.http_timeout_secs));
        for (k, v) in &a.headers {
            rb = rb.header(k, v);
        }
        if let Some(b) = a.body {
            rb = rb.body(b);
        }
        let resp = tokio::select! {
            r = rb.send() => r.map_err(|e| ToolError::Failed(e.to_string()))?,
            _ = ctx.cancel.cancelled() => return Err(ToolError::Cancelled),
        };
        let status = resp.status();
        let ctype = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let body = resp
            .text()
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        let body = if ctype.contains("html") {
            html_to_text(&body)
        } else {
            body
        };
        let out = ToolOutput {
            content: vec![gai_core::ContentBlock::text(format!(
                "HTTP {status}\ncontent-type: {ctype}\n\n{body}"
            ))],
            is_error: !status.is_success(),
            cost: None,
        };
        Ok(out.truncated(ctx.settings.max_output_chars))
    }
}

/// True when `host` is, or resolves to, a loopback, private, link-local or
/// otherwise non-public address.
pub fn is_private_host(host: &str, port: u16) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host.ends_with(".internal")
    {
        return true;
    }
    let addrs: Vec<IpAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => match (host, port).to_socket_addrs() {
            Ok(it) => it.map(|s| s.ip()).collect(),
            Err(_) => return false, // unresolvable: let the request fail normally
        },
    };
    addrs.iter().any(|ip| match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]) // CGNAT
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // unique local
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link local
                || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback() || v4.is_private())
        }
    })
}

/// A rough HTML-to-text pass: drop scripts, styles and tags, decode common
/// entities, collapse whitespace. Enough for a model to read a page.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 3);
    let lower = html.to_lowercase();
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'<' {
            for skip in ["script", "style", "noscript", "svg"] {
                if lower[i + 1..].starts_with(skip) {
                    let close = format!("</{skip}");
                    i = lower[i..]
                        .find(&close)
                        .map(|p| i + p)
                        .unwrap_or(bytes.len());
                    break;
                }
            }
            let end = html[i..]
                .find('>')
                .map(|p| i + p + 1)
                .unwrap_or(bytes.len());
            let tag = &lower[i..end.min(lower.len())];
            if [
                "<br", "<p", "</p", "<div", "</div", "<li", "<h1", "<h2", "<h3", "<tr", "</h",
            ]
            .iter()
            .any(|t| tag.starts_with(t))
            {
                out.push('\n');
            }
            i = end;
        } else {
            let next = html[i..].find('<').map(|p| i + p).unwrap_or(bytes.len());
            out.push_str(&html[i..next]);
            i = next;
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let mut lines: Vec<String> = Vec::new();
    for line in decoded.lines() {
        let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !collapsed.is_empty() || lines.last().is_some_and(|l| !l.is_empty()) {
            lines.push(collapsed);
        }
    }
    lines.join("\n").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn private_hosts() {
        assert!(is_private_host("localhost", 80));
        assert!(is_private_host("127.0.0.1", 80));
        assert!(is_private_host("10.1.2.3", 80));
        assert!(is_private_host("[::1]", 80));
        assert!(is_private_host("169.254.169.254", 80));
        assert!(!is_private_host("93.184.216.34", 80));
    }

    #[test]
    fn html_becomes_text() {
        let t = html_to_text(
            "<html><head><style>x{}</style></head><body><h1>Title</h1><p>Hello &amp; <b>bye</b></p><script>evil()</script></body></html>",
        );
        assert_eq!(t, "Title\n\nHello & bye");
    }

    #[tokio::test]
    async fn fetches_when_private_allowed_and_refuses_otherwise() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            // set_body_raw sets the content type; set_body_string would force text/plain.
            .respond_with(ResponseTemplate::new(200).set_body_raw("<p>hi</p>", "text/html"))
            .mount(&server)
            .await;
        let mut ctx = ToolCtx::for_workspace(".");
        let err = HttpFetch
            .call(&ctx, json!({"url": server.uri()}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("private"));
        ctx.settings = std::sync::Arc::new(crate::ToolSettings {
            allow_private_network: true,
            ..Default::default()
        });
        let out = HttpFetch
            .call(&ctx, json!({"url": server.uri()}))
            .await
            .unwrap();
        assert!(out.as_text().starts_with("HTTP 200 OK"));
        assert!(out.as_text().ends_with("hi"));
    }
}
