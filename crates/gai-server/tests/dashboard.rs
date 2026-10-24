//! The dashboard built into the binary: the app at `/` and at client-side
//! routes, cached assets, and 404s (not the app) for unknown API paths.

use std::sync::Arc;

use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, Dashboard, router};

async fn start(dashboard: Dashboard) -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        data_dir: Some(dir.path().to_path_buf()),
        default_model: Some("mock/echo".into()),
        ..Default::default()
    };
    let gai = Arc::new(Gai::from_config(config, None).unwrap());
    let app = router(gai, AuthPolicy::new(AuthMode::Off, true, 0), dashboard)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, dir)
}

#[tokio::test]
async fn embedded_dashboard_serves_the_app() {
    let (url, _dir) = start(Dashboard::Embedded).await;
    for path in ["/", "/index.html", "/runs/run_123", "/settings"] {
        let r = reqwest::get(format!("{url}{path}")).await.unwrap();
        assert_eq!(r.status(), 200, "{path}");
        assert_eq!(r.headers()["content-type"], "text/html", "{path}");
        assert_eq!(r.headers()["cache-control"], "no-cache", "{path}");
        assert!(r.text().await.unwrap().contains("<html"), "{path}");
    }
    for path in ["/v1/nope", "/openai/v1/nope", "/.well-known/nope"] {
        let r = reqwest::get(format!("{url}{path}")).await.unwrap();
        assert_eq!(r.status(), 404, "{path}");
    }
    // The API still answers.
    let r = reqwest::get(format!("{url}/v1/health")).await.unwrap();
    assert!(r.status().is_success());
}

#[tokio::test]
async fn no_dashboard_means_404() {
    let (url, _dir) = start(Dashboard::Off).await;
    let r = reqwest::get(format!("{url}/")).await.unwrap();
    assert_eq!(r.status(), 404);
}
