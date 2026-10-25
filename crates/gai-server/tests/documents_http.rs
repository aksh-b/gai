//! Extracting fields from a collection's documents into a table, over HTTP.

use std::sync::Arc;

use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use serde_json::{Value, json};

async fn server() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        data_dir: Some(dir.path().to_path_buf()),
        default_model: Some("mock/fields".into()),
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

#[tokio::test]
async fn a_collection_becomes_a_table() {
    let (u, _dir) = server().await;
    let c = reqwest::Client::new();
    let r = c
        .post(format!("{u}/v1/collections"))
        .json(&json!({"name": "contracts"}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.text().await.unwrap());
    for (name, text) in [
        (
            "acme.md",
            "# Acme supply agreement\n\nParty: Acme GmbH\nEnd date: 2026-12-31\nMonthly fee: 4,000 EUR\n",
        ),
        (
            "globex.md",
            "# Globex lease\n\nParty: Globex Inc.\nMonthly fee: 2,500 USD\n",
        ),
    ] {
        let r = c
            .post(format!("{u}/v1/collections/contracts/documents"))
            .json(&json!({"source": name, "title": text.lines().next().unwrap().trim_start_matches("# "), "text": text}))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "{}", r.text().await.unwrap());
    }
    let fields = json!({"party": "the other party", "end_date": "when it ends", "monthly_fee": "the fee per month"});
    let r = c
        .post(format!("{u}/v1/collections/contracts/extract"))
        .json(&json!({"fields": fields}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    let rows = v["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{v}");
    assert_eq!(rows[0]["source"], "acme.md");
    assert_eq!(rows[0]["fields"]["party"], "Acme GmbH");
    assert_eq!(rows[0]["fields"]["end_date"], "2026-12-31");
    assert_eq!(rows[1]["fields"]["monthly_fee"], "2,500 USD");
    assert_eq!(rows[1]["fields"]["end_date"], Value::Null);

    let r = c
        .post(format!("{u}/v1/collections/contracts/extract?format=csv"))
        .json(&json!({"fields": fields, "documents": ["Globex lease"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()["content-type"], "text/csv; charset=utf-8");
    let csv = r.text().await.unwrap();
    assert_eq!(csv.lines().count(), 2, "{csv}");
    assert!(csv.contains("Globex Inc."), "{csv}");

    let r = c
        .post(format!("{u}/v1/collections/contracts/extract"))
        .json(&json!({"fields": {}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}
