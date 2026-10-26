//! Dashboards over a SQLite database: refreshing, a broken panel, and a
//! KPI alert that emails once, not at every refresh, and again after it
//! recovers.

use std::sync::Arc;

use gai_data::Data;
use gai_data::dashboards::{Chart, Dashboards};
use gai_store::{SharedStore, SqliteStore};
use gai_tools::notify::{Notifier, NotifySettings};
use serde_json::json;

async fn setup(dir: &std::path::Path) -> (Dashboards, sqlx::SqlitePool) {
    let db = dir.join("app.db");
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&db)
        .create_if_missing(true);
    let pool = sqlx::SqlitePool::connect_with(opts).await.unwrap();
    sqlx::query("CREATE TABLE signups (day TEXT, n INTEGER)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO signups VALUES ('2026-10-01', 120), ('2026-10-02', 110), ('2026-10-03', 40)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let data = Data::new(store);
    data.save(&Data::parse(json!({"name": "app", "kind": "sqlite", "path": db})).unwrap())
        .await
        .unwrap();
    let notifier = Notifier::new(
        NotifySettings {
            smtp_url: Some(format!("file://{}", dir.join("mail").display())),
            ..Default::default()
        },
        true,
    );
    (Dashboards::new(data, Some(notifier)), pool)
}

fn mails(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir.join("mail"))
        .map(|d| d.count())
        .unwrap_or(0)
}

#[tokio::test]
async fn refresh_and_alert_once() {
    let dir = tempfile::tempdir().unwrap();
    let (d, pool) = setup(dir.path()).await;
    let spec = Dashboards::parse(json!({
        "name": "growth", "title": "Growth", "refresh_minutes": 60,
        "notify": {"email": ["ana@example.com"]},
        "panels": [
            {"title": "Signups per day", "connection": "app", "chart": "line", "x": "day", "y": ["n"],
             "sql": "SELECT day, n FROM signups ORDER BY day", "alert": {"below": 50}},
            {"title": "Total", "connection": "app", "chart": "number", "sql": "SELECT SUM(n) AS total FROM signups"},
            {"title": "Broken", "connection": "app", "sql": "SELECT nope FROM signups"}
        ]
    }))
    .unwrap();
    assert_eq!(spec.panels[1].chart, Chart::Number);
    d.save(&spec).await.unwrap();
    assert_eq!(d.due().await.unwrap(), ["growth"], "never refreshed");

    let r = d.refresh("growth").await.unwrap();
    assert_eq!(r.panels[0].rows.len(), 3);
    assert_eq!(
        (r.panels[0].value, r.panels[0].alerting),
        (Some(40.0), true)
    );
    assert_eq!(r.panels[1].rows[0][0], json!(270));
    assert!(r.panels[2].error.is_some());
    assert_eq!(r.alerts, ["Signups per day: 40 is below 50"]);
    assert_eq!(mails(dir.path()), 1);
    assert!(d.due().await.unwrap().is_empty(), "refreshed just now");

    // Still low: no second email.
    let r = d.refresh("growth").await.unwrap();
    assert!(r.panels[0].alerting && r.alerts.is_empty());
    assert_eq!(mails(dir.path()), 1);
    // Back up, then down again: a new alert.
    sqlx::query("INSERT INTO signups VALUES ('2026-10-04', 90)")
        .execute(&pool)
        .await
        .unwrap();
    assert!(!d.refresh("growth").await.unwrap().panels[0].alerting);
    sqlx::query("INSERT INTO signups VALUES ('2026-10-05', 10)")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(d.refresh("growth").await.unwrap().alerts.len(), 1);
    assert_eq!(mails(dir.path()), 2);

    let v = d.get("growth").await.unwrap();
    assert_eq!(v.results.unwrap().panels[0].rows.len(), 5);
    assert_eq!(d.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn bad_dashboards_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (d, _pool) = setup(dir.path()).await;
    let panel = |sql: &str, conn: &str| json!({"name": "x", "title": "X", "panels": [{"title": "p", "connection": conn, "sql": sql}]});
    assert!(
        d.save(&Dashboards::parse(panel("DELETE FROM signups", "app")).unwrap())
            .await
            .is_err()
    );
    assert!(
        d.save(&Dashboards::parse(panel("SELECT 1", "nope")).unwrap())
            .await
            .is_err()
    );
    assert!(Dashboards::parse(json!({"name": "x", "title": "X", "panels": []})).is_err());
    assert!(Dashboards::parse(json!({"name": "x", "title": "X", "panels": [{"title": "p", "connection": "app", "sql": "SELECT 1", "alert": {}}]})).is_err());
    assert!(Dashboards::parse(json!({"name": "x", "title": "X", "notify": {"email": ["nope"]}, "panels": [{"title": "p", "connection": "app", "sql": "SELECT 1"}]})).is_err());

    // The tool saves and refreshes in one go.
    let tool = d.tool();
    let out = tool
        .call(
            &gai_tools::ToolCtx::for_workspace(dir.path()),
            json!({"name": "kpis", "title": "KPIs", "panels": [{"title": "Days", "connection": "app", "chart": "number", "sql": "SELECT COUNT(*) AS days FROM signups"}]}),
        )
        .await
        .unwrap();
    let text: String = out.content.iter().filter_map(|b| b.as_text()).collect();
    assert!(
        text.contains("#/dashboards/kpis") && text.contains("Days: 1 rows"),
        "{text}"
    );
}
