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

/// Notification rules: a finished run and an approval each send an email,
/// a rule can be tested, and browsers can subscribe to web push.
#[tokio::test]
async fn notification_rules() {
    let dir = tempfile::tempdir().unwrap();
    let mail = dir.path().join("mail");
    let mut config = Config {
        data_dir: Some(dir.path().join("data")),
        default_model: Some("mock/echo".into()),
        ..Default::default()
    };
    config.notify.smtp_url = Some(format!("file://{}", mail.display()));
    config.notify.dashboard_url = Some("https://gai.example.com".into());
    let gai = Arc::new(Gai::from_config(config, None).unwrap());
    let app = router(gai, AuthPolicy::new(AuthMode::Off, true, 0), None)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let c = reqwest::Client::new();
    let post = |path: &str, body: Value| c.post(format!("{url}{path}")).json(&body).send();
    let mails = |want: usize| {
        let mail = mail.clone();
        async move {
            for _ in 0..250 {
                let texts: Vec<String> = std::fs::read_dir(&mail)
                    .map(|d| {
                        d.filter_map(|e| std::fs::read_to_string(e.ok()?.path()).ok())
                            .collect()
                    })
                    .unwrap_or_default();
                if texts.len() >= want {
                    return texts;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("expected {want} emails");
        }
    };

    let r = post(
        "/v1/notifications/rules",
        json!({"yaml": "name: oncall\non: [approval, run_finished]\nemail: [ops@example.com]\n"}),
    )
    .await
    .unwrap();
    assert_eq!(r.status(), 201);
    let bad = post("/v1/notifications/rules", json!({"name": "x", "on": []}))
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    let r = post("/v1/runs", json!({"input": "hello there"}))
        .await
        .unwrap();
    assert!(r.status().is_success());
    let got = mails(1).await;
    assert!(
        got[0].contains("Run finished") && got[0].contains("https://gai.example.com/#/runs/"),
        "{}",
        got[0]
    );

    let r = post(
        "/v1/runs",
        json!({"input": "call:shell {\"command\": \"echo hi\"}", "mode": "background"}),
    )
    .await
    .unwrap();
    assert!(r.status().is_success());
    let got = mails(2).await;
    assert!(
        got.iter().any(|m| m.contains("Approval needed: shell")),
        "{got:?}"
    );

    let sent: Value = post("/v1/notifications/rules/oncall/test", json!({}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sent["to"][0], "email to ops@example.com");

    let info: Value = c
        .get(format!("{url}/v1/notifications/push"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["public_key"].as_str().unwrap().len(), 87);
    let sub = json!({
        "endpoint": "https://push.example.com/send/abc",
        "keys": {
            "p256dh": "BNcRdreALRFXTkOOUHK1EtK2wtaz5Ry4YfYCA_0QTpQtUbVlUls0VJXg7A8u-Ts1XbjhazAkj7I99e8QcYP7DkM",
            "auth": "tBHItJI5svbpez7KI4CCXg"
        }
    });
    let d: Value = post("/v1/notifications/push", sub)
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(d["endpoint"], "https://push.example.com/send/abc");
    let r: Value = post("/v1/notifications/push/unsubscribe", json!({"id": d["id"]}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["ok"], true);
}

/// Suggestions: an agent suggests running another, gai suggests an
/// assistant for a database nothing uses, and both can be accepted.
#[tokio::test]
async fn suggestions_inbox() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("app.db");
    {
        let opts = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&db)
            .create_if_missing(true);
        let pool = sqlx::SqlitePool::connect_with(opts).await.unwrap();
        sqlx::query("CREATE TABLE t (x INTEGER)")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }
    let config = Config {
        data_dir: Some(dir.path().join("data")),
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
    let c = reqwest::Client::new();
    let post = |path: &str, body: Value| c.post(format!("{url}{path}")).json(&body).send();
    let list = || async {
        c.get(format!("{url}/v1/suggestions"))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };

    for (name, tools) in [("watcher", json!(["suggest"])), ("buyer", json!([]))] {
        let r = post(
            "/v1/agents",
            json!({"name": name, "model": "mock/echo", "tools": tools}),
        )
        .await
        .unwrap();
        assert!(r.status().is_success());
    }
    let call = json!({
        "title": "Restock SKU 1182",
        "details": "Only 3 left; 40 sold last week.",
        "action": {"kind": "run_agent", "agent": "buyer", "input": "Draft a purchase order for SKU 1182"}
    });
    let r = post(
        "/v1/runs",
        json!({"agent": "watcher", "input": format!("call:suggest {call}")}),
    )
    .await
    .unwrap();
    assert!(r.status().is_success());
    let open = list().await;
    let mine = open
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Restock SKU 1182")
        .cloned()
        .expect("the agent's suggestion");
    assert_eq!(mine["source"], "watcher");
    let id = mine["id"].as_str().unwrap();
    let a: Value = post(&format!("/v1/suggestions/{id}/accept"), json!({}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(a["run_id"].as_str().is_some(), "{a}");
    assert_eq!(a["suggestion"]["status"], "accepted");
    let again = post(&format!("/v1/suggestions/{id}/accept"), json!({}))
        .await
        .unwrap();
    assert_eq!(again.status(), 409);

    let r = post(
        "/v1/connections",
        json!({"name": "app", "kind": "sqlite", "path": db}),
    )
    .await
    .unwrap();
    assert_eq!(r.status(), 201);
    let open = list().await;
    let s = open
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "gai-sql-analyst")
        .cloned()
        .expect("a suggestion for the database");
    assert_eq!(s["action"]["template"], "sql-analyst");
    let a: Value = post("/v1/suggestions/gai-sql-analyst/accept", json!({}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(a["installed"], "sql-analyst");
    assert_eq!(a["link"], "#/assistants/sql-analyst");

    // A dismissed suggestion from usage is not made again.
    let r = post(
        "/v1/connections",
        json!({"name": "app2", "kind": "sqlite", "path": db}),
    )
    .await
    .unwrap();
    assert_eq!(r.status(), 201);
    assert!(
        list()
            .await
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["status"] == "open" && s["id"] != "gai-sql-analyst"),
    );
    let missing = post("/v1/suggestions/nope/dismiss", json!({}))
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}
