//! The dashboard: built into the binary, or served from a folder.

use std::path::PathBuf;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use tower_http::services::{ServeDir, ServeFile};

/// Where the dashboard comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Dashboard {
    /// The build of `web/` compiled into the binary.
    #[default]
    Embedded,
    /// A build on disk (`web/dist`), for working on the dashboard.
    Dir(PathBuf),
    /// No dashboard: only the API.
    Off,
}

impl From<Option<PathBuf>> for Dashboard {
    fn from(dir: Option<PathBuf>) -> Self {
        dir.map_or(Dashboard::Off, Dashboard::Dir)
    }
}

impl Dashboard {
    /// Whether there is a real dashboard to show (not the "not built" page).
    pub fn available(&self) -> bool {
        match self {
            Dashboard::Embedded => Assets::get("index.html")
                .is_some_and(|f| !String::from_utf8_lossy(&f.data).contains("gai-placeholder")),
            Dashboard::Dir(d) => d.join("index.html").is_file(),
            Dashboard::Off => false,
        }
    }

    /// Add the dashboard as the fallback for paths no route matched.
    pub fn mount(&self, app: Router) -> Router {
        match self {
            Dashboard::Embedded => app.fallback(embedded),
            Dashboard::Dir(dir) if dir.join("index.html").is_file() => {
                // Client-side routes fall back to index.html.
                let spa = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
                app.fallback_service(spa)
            }
            _ => app,
        }
    }
}

#[derive(rust_embed::Embed)]
#[folder = "$GAI_UI_EMBED_DIR"]
struct Assets;

/// API-looking paths that matched no route get a 404, not the dashboard.
fn is_api(path: &str) -> bool {
    ["v1/", "openai/", "mcp", "a2a/", ".well-known/"]
        .iter()
        .any(|p| path.starts_with(p))
}

async fn embedded(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if is_api(path) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let (file, name) = match Assets::get(path).filter(|_| !path.is_empty()) {
        Some(f) => (f, path),
        // Client-side routes (and `/`) get the app.
        None => match Assets::get("index.html") {
            Some(f) => (f, "index.html"),
            None => return StatusCode::NOT_FOUND.into_response(),
        },
    };
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    // Vite puts a content hash in asset names, so those never change.
    let cache = if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mut resp = Response::new(Body::from(file.data.into_owned()));
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(mime.essence_str()) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    resp
}
