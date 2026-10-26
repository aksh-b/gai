//! Shared HTTP plumbing for dialects: auth, extra headers and query, error
//! mapping and SSE decoding.

use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value};

use crate::{LlmError, ProviderConfig};

#[derive(Clone, Debug)]
pub struct HttpTarget {
    client: reqwest::Client,
    base_url: String,
    headers: HeaderMap,
    query: Vec<(String, String)>,
    extra_body: Map<String, Value>,
}

impl HttpTarget {
    /// Build from provider config. `default_auth` is the dialect's usual
    /// header and scheme, used when the config does not override them.
    pub fn new(
        provider: &str,
        cfg: &ProviderConfig,
        default_base: &str,
        default_auth: (&str, &str),
    ) -> Result<Self, LlmError> {
        let mut headers = HeaderMap::new();
        if let Some(key) = cfg.require_key(provider)? {
            let name = cfg.auth_header.as_deref().unwrap_or(default_auth.0);
            let scheme = cfg.auth_scheme.as_deref().unwrap_or(default_auth.1);
            let value = if scheme.is_empty() {
                key
            } else {
                format!("{scheme} {key}")
            };
            let mut value = HeaderValue::from_str(&value).map_err(|_| {
                LlmError::Config(format!(
                    "API key for {provider:?} contains invalid characters"
                ))
            })?;
            value.set_sensitive(true);
            headers.insert(header_name(name)?, value);
        }
        for (k, v) in &cfg.headers {
            let v = HeaderValue::from_str(v)
                .map_err(|_| LlmError::Config(format!("invalid header value for {k}")))?;
            headers.insert(header_name(k)?, v);
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(cfg.timeout())
            .user_agent(concat!("gai/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| LlmError::Config(e.to_string()))?;
        Ok(Self {
            client,
            base_url: cfg
                .base_url
                .as_deref()
                .unwrap_or(default_base)
                .trim_end_matches('/')
                .to_owned(),
            headers,
            query: cfg
                .query
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            extra_body: cfg.extra_body.clone(),
        })
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// The WebSocket address for `path` (`https` becomes `wss`), with the
    /// configured query.
    pub fn ws_url(&self, path: &str) -> String {
        let url = self.url(path);
        let url = match url.split_once("://") {
            Some(("https", rest)) => format!("wss://{rest}"),
            Some(("http", rest)) => format!("ws://{rest}"),
            _ => url,
        };
        match reqwest::Url::parse(&url) {
            Ok(mut u) => {
                for (k, v) in &self.query {
                    u.query_pairs_mut().append_pair(k, v);
                }
                u.to_string()
            }
            Err(_) => url,
        }
    }

    /// The headers every request carries (auth included).
    pub fn header_pairs(&self) -> Vec<(String, String)> {
        self.headers
            .iter()
            .filter_map(|(k, v)| Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned())))
            .collect()
    }

    pub fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .get(self.url(path))
            .headers(self.headers.clone())
            .query(&self.query)
    }

    /// POST a JSON body after merging the provider's and the request's extra
    /// fields into it.
    pub fn post_json(
        &self,
        path: &str,
        mut body: Value,
        extra: &Map<String, Value>,
    ) -> reqwest::RequestBuilder {
        if let Value::Object(obj) = &mut body {
            for (k, v) in self.extra_body.iter().chain(extra) {
                obj.insert(k.clone(), v.clone());
            }
        }
        self.client
            .post(self.url(path))
            .headers(self.headers.clone())
            .query(&self.query)
            .json(&body)
    }
}

impl HttpTarget {
    /// POST a multipart form (file uploads).
    pub fn post_multipart(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> reqwest::RequestBuilder {
        self.client
            .post(self.url(path))
            .headers(self.headers.clone())
            .query(&self.query)
            .multipart(form)
    }

    /// GET an absolute URL (e.g. an image a provider links to) with this
    /// target's client, without its auth headers.
    pub fn fetch(&self, url: &str) -> reqwest::RequestBuilder {
        self.client.get(url)
    }
}

fn header_name(name: &str) -> Result<HeaderName, LlmError> {
    HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| LlmError::Config(format!("invalid header name {name:?}")))
}

/// Send and turn non-2xx responses into typed errors.
pub async fn send(rb: reqwest::RequestBuilder) -> Result<reqwest::Response, LlmError> {
    let resp = rb.send().await?;
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map(Duration::from_secs_f64);
    let body = resp.text().await.unwrap_or_default();
    Err(LlmError::from_status(status.as_u16(), &body, retry_after))
}

/// A decoded server-sent event.
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

pub fn sse(resp: reqwest::Response) -> impl Stream<Item = Result<SseEvent, LlmError>> + Send {
    resp.bytes_stream().eventsource().map(|r| match r {
        Ok(e) => Ok(SseEvent {
            event: e.event,
            data: e.data,
        }),
        Err(e) => Err(LlmError::Network(e.to_string())),
    })
}
