//! Sources over HTTP: save, sync, list, delete.

use std::sync::Arc;

use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use reqwest::Method;
use serde_json::{Value, json};

async fn server() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        data_dir: Some(dir.path().to_path_buf()),
        default_model: Some("mock/echo".into()),
        ..Default::default()
    };
    let gai = Arc::new(Gai::from_config(config, None).unwrap());
    let app = router(gai, AuthPolicy::new(AuthMode::Off, true, 0), None)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, dir)
}

async fn call(method: Method, url: &str, body: Option<Value>) -> (u16, Value) {
    let mut rb = reqwest::Client::new().request(method, url);
    if let Some(b) = body {
        rb = rb.json(&b);
    }
    let r = rb.send().await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_folder_source_syncs_over_http() {
    let (u, dir) = server().await;
    let docs = dir.path().join("docs");
    std::fs::create_dir(&docs).unwrap();
    std::fs::write(docs.join("a.md"), "# Alpha\n\nThe alpha guide.").unwrap();
    std::fs::write(docs.join("b.txt"), "Bravo notes.").unwrap();

    let yaml = format!(
        "name: handbook\ncollection: docs\nkind: folder\npath: {}\n",
        docs.display()
    );
    let (s, rec) = call(
        Method::POST,
        &format!("{u}/v1/sources"),
        Some(json!({"yaml": yaml})),
    )
    .await;
    assert_eq!(s, 201, "{rec}");
    assert_eq!(rec["status"], "new");
    assert!(rec.get("state").is_none());

    let (s, stats) = call(
        Method::POST,
        &format!("{u}/v1/sources/handbook/sync?wait=true"),
        None,
    )
    .await;
    assert_eq!(s, 200, "{stats}");
    assert_eq!(stats["added"], 2);

    let (s, list) = call(Method::GET, &format!("{u}/v1/sources"), None).await;
    assert_eq!(s, 200);
    assert_eq!(list[0]["status"], "ok");
    assert_eq!(list[0]["stats"]["added"], 2);

    // Background sync: nothing changed.
    let (s, _) = call(Method::POST, &format!("{u}/v1/sources/handbook/sync"), None).await;
    assert_eq!(s, 202);

    let (s, err) = call(
        Method::POST,
        &format!("{u}/v1/sources"),
        Some(json!({"name": "x", "collection": "c", "kind": "ftp"})),
    )
    .await;
    assert_eq!(s, 400, "{err}");

    for _ in 0..50 {
        let (_, r) = call(Method::GET, &format!("{u}/v1/sources/handbook"), None).await;
        if r["status"] == "ok" && r["stats"]["unchanged"] == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let (s, _) = call(Method::DELETE, &format!("{u}/v1/sources/handbook"), None).await;
    assert_eq!(s, 200);
    let (s, _) = call(Method::GET, &format!("{u}/v1/sources/handbook"), None).await;
    assert_eq!(s, 404);
}
