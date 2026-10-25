//! Bundles over HTTP: export from one server, inspect and import into
//! another, and the admin scope they need.

use std::sync::Arc;

use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use serde_json::{Value, json};

async fn server(mode: AuthMode) -> (String, Arc<Gai>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        data_dir: Some(dir.path().to_path_buf()),
        default_model: Some("mock/echo".into()),
        ..Default::default()
    };
    let gai = Arc::new(Gai::from_config(config, None).unwrap());
    let app = router(gai.clone(), AuthPolicy::new(mode, true, 0), None)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, gai, dir)
}

#[tokio::test]
async fn export_then_import_over_http() {
    let (a, gai_a, _da) = server(AuthMode::Off).await;
    let (b, gai_b, _db) = server(AuthMode::Off).await;
    let http = reqwest::Client::new();
    let spec = json!({"name": "poet", "system": "Write poems.", "tools": ["calculator"]});
    let r = http
        .post(format!("{a}/v1/agents"))
        .json(&spec)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());

    let r = http
        .post(format!("{a}/v1/bundles/export"))
        .json(&json!({"name": "Poetry kit", "all": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "application/zip");
    assert!(
        r.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("Poetry-kit.gaibundle")
    );
    let bundle = r.bytes().await.unwrap();

    let peek: Value = http
        .post(format!("{b}/v1/bundles/inspect"))
        .body(bundle.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(peek["manifest"]["contents"]["agents"], json!(["poet"]));
    let report: Value = http
        .post(format!("{b}/v1/bundles/import"))
        .body(bundle.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(report["installed"], json!(["agent/poet@v1"]), "{report}");
    let report: Value = http
        .post(format!("{b}/v1/bundles/import?skip_existing=true"))
        .body(bundle)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(report["skipped"], json!(["agent/poet"]), "{report}");
    assert_eq!(
        gai_a.find_agent("poet").await.unwrap().0,
        gai_b.find_agent("poet").await.unwrap().0
    );

    // Garbage is a 400, nothing to export too.
    let r = http
        .post(format!("{b}/v1/bundles/import"))
        .body("nope")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let r = http
        .post(format!("{b}/v1/bundles/export"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}

#[test]
fn bundles_need_admin() {
    use gai::store::Scope;
    use gai_server::required_scope;
    use reqwest::Method;
    for p in [
        "/v1/bundles/export",
        "/v1/bundles/import",
        "/v1/bundles/inspect",
    ] {
        assert_eq!(required_scope(&Method::POST, p), Scope::Admin, "{p}");
    }
}
