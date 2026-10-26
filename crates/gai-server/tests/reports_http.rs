//! Dashboards over HTTP, and a scheduled report: a trigger whose run's
//! answer is emailed (to a mail folder here) when it finishes.

use std::sync::Arc;
use std::time::Duration;

use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use serde_json::{Value, json};

#[tokio::test]
async fn dashboards_and_reports() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("app.db");
    {
        let opts = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&db)
            .create_if_missing(true);
        let pool = sqlx::SqlitePool::connect_with(opts).await.unwrap();
        sqlx::query("CREATE TABLE signups (day TEXT, n INTEGER)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO signups VALUES ('2026-10-01', 120), ('2026-10-02', 30)")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }
    let mail = dir.path().join("mail");
    let mut config = Config {
        data_dir: Some(dir.path().join("data")),
        default_model: Some("mock/echo".into()),
        ..Default::default()
    };
    config.notify.smtp_url = Some(format!("file://{}", mail.display()));
    let gai = Arc::new(Gai::from_config(config, None).unwrap());
    let app = router(gai, AuthPolicy::new(AuthMode::Off, true, 0), None)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let c = reqwest::Client::new();
    let post = |path: &str, body: Value| c.post(format!("{url}{path}")).json(&body).send();

    let r = post(
        "/v1/connections",
        json!({"name": "app", "kind": "sqlite", "path": db}),
    )
    .await
    .unwrap();
    assert_eq!(r.status(), 201);
    let yaml = "name: growth\ntitle: Growth\nnotify: { email: [ana@example.com] }\npanels:\n  - title: Signups\n    connection: app\n    sql: SELECT day, n FROM signups ORDER BY day\n    chart: bar\n    x: day\n    y: [n]\n    alert: { below: 50 }\n";
    let r = post("/v1/dashboards", json!({"yaml": yaml})).await.unwrap();
    assert_eq!(r.status(), 201);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["spec"]["panels"][0]["chart"], "bar");
    assert_eq!(v["results"]["panels"][0]["alerting"], true);
    assert_eq!(
        std::fs::read_dir(&mail).unwrap().count(),
        1,
        "the alert was emailed"
    );
    let r: Value = post("/v1/dashboards/growth/refresh", json!({}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["alerts"], Value::Null, "not again while it stays low");
    let list: Value = c
        .get(format!("{url}/v1/dashboards"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    let bad = post("/v1/dashboards", json!({"name": "x", "title": "X", "panels": [{"title": "p", "connection": "app", "sql": "DROP TABLE signups"}]})).await.unwrap();
    assert_eq!(bad.status(), 400);

    // A report: the trigger's run is emailed with its subject.
    let r = post(
        "/v1/agents",
        json!({"name": "reporter", "model": "mock/echo"}),
    )
    .await
    .unwrap();
    assert_eq!(r.status(), 201);
    let created: Value = post(
        "/v1/triggers",
        json!({
            "name": "weekly-signups", "kind": "webhook", "target": {"agent": "reporter"},
            "input": "Signups fell 75% this week.",
            "deliver": {"email": ["boss@example.com"], "subject": "Weekly signups"}
        }),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let id = created["trigger"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{created}"))
        .to_owned();
    let r = post(&format!("/v1/triggers/{id}/fire"), json!({}))
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.text().await.unwrap());
    let mut found = None;
    for _ in 0..100 {
        let mails: Vec<_> = std::fs::read_dir(&mail)
            .unwrap()
            .flatten()
            .map(|e| std::fs::read_to_string(e.path()).unwrap())
            .collect();
        if let Some(m) = mails.into_iter().find(|m| m.contains("Weekly signups")) {
            found = Some(m);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let m = found.expect("the report was emailed");
    assert!(
        m.contains("boss@example.com") && m.contains("Signups fell 75% this week."),
        "{m}"
    );
    let bad = post(
        "/v1/triggers",
        json!({"name": "bad", "kind": "webhook", "target": {"agent": "reporter"}, "deliver": {"email": ["not-an-address"]}}),
    )
    .await
    .unwrap();
    assert_eq!(bad.status(), 400);
}
