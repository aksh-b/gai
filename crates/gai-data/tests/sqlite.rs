//! A SQLite connection end to end: schema, queries, limits, refusals, dry
//! runs, writes on writable connections, and the tools.

use std::sync::Arc;

use gai_data::{Data, DataError};
use gai_store::{SharedStore, SqliteStore};
use gai_tools::ToolCtx;
use serde_json::json;

/// Signups per day for two weeks: `ads` stops in the second week.
async fn signups_db(path: &std::path::Path) {
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    let pool = sqlx::SqlitePool::connect_with(opts).await.unwrap();
    sqlx::query("CREATE TABLE signups (id INTEGER PRIMARY KEY, day TEXT NOT NULL, channel TEXT NOT NULL, plan TEXT)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE VIEW daily AS SELECT day, COUNT(*) AS n FROM signups GROUP BY day")
        .execute(&pool)
        .await
        .unwrap();
    let mut id = 0;
    for day in 1..=14 {
        for (channel, n) in [
            ("organic", 10),
            ("ads", if day <= 7 { 8 } else { 1 }),
            ("referral", 3),
        ] {
            for _ in 0..n {
                id += 1;
                sqlx::query("INSERT INTO signups (id, day, channel, plan) VALUES (?, ?, ?, ?)")
                    .bind(id)
                    .bind(format!("2026-10-{day:02}"))
                    .bind(channel)
                    .bind(if id % 3 == 0 { Some("pro") } else { None })
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }
    }
    pool.close().await;
}

async fn data(dir: &std::path::Path, read_only: bool) -> Data {
    let db = dir.join("app.db");
    if !db.exists() {
        signups_db(&db).await;
    }
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let d = Data::new(store);
    let spec = Data::parse(json!({
        "name": "app", "kind": "sqlite", "path": db, "read_only": read_only, "max_rows": 20,
        "description": "Signups by day and channel",
    }))
    .unwrap();
    d.save(&spec).await.unwrap();
    d
}

#[tokio::test]
async fn schema_queries_and_refusals() {
    let dir = tempfile::tempdir().unwrap();
    let d = data(dir.path(), true).await;

    let s = d.schema("app", false).await.unwrap();
    let signups = s.tables.iter().find(|t| t.name == "signups").unwrap();
    assert_eq!(signups.rows, Some(14 * 21 - 7 * 7));
    assert_eq!(signups.columns.len(), 4);
    assert!(!signups.columns[1].nullable && signups.columns[3].nullable);
    assert_eq!(
        s.tables.iter().find(|t| t.name == "daily").unwrap().kind,
        "view"
    );
    assert!(
        s.describe(&[]).contains("signups (~245 rows): id integer"),
        "{}",
        s.describe(&[])
    );
    // Kept for next time.
    assert!(d.get("app").await.unwrap().schema.is_some());

    let r = d
        .query(
            "app",
            "SELECT channel, SUM(day > '2026-10-07') AS last_week, SUM(day <= '2026-10-07') AS week_before
             FROM signups GROUP BY channel ORDER BY channel",
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        r.columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["channel", "last_week", "week_before"]
    );
    assert_eq!(r.rows[0], [json!("ads"), json!(7), json!(56)]);
    assert!(!r.truncated);
    assert!(r.markdown(10).contains("| ads | 7 | 56 |"));
    assert!(
        r.csv()
            .starts_with("channel,last_week,week_before\nads,7,56\n")
    );

    // Limits: the connection's max_rows, or less.
    let r = d.query("app", "SELECT * FROM signups", None).await.unwrap();
    assert_eq!((r.rows.len(), r.truncated), (20, true));
    assert_eq!(r.rows[0][3], json!(null));
    let r = d
        .query("app", "SELECT * FROM signups", Some(5))
        .await
        .unwrap();
    assert_eq!(r.rows.len(), 5);

    for bad in [
        "DELETE FROM signups",
        "SELECT 1; DELETE FROM signups",
        "DROP VIEW daily",
    ] {
        assert!(
            matches!(d.query("app", bad, None).await, Err(DataError::Refused(_))),
            "{bad}"
        );
    }
    let e = d
        .query("app", "SELECT nope FROM signups", None)
        .await
        .unwrap_err();
    assert!(matches!(e, DataError::Database(_)), "{e}");
    assert!(
        d.query("app", "SELECT 1 AS a, 2 AS a", None).await.is_err(),
        "duplicate column names"
    );
    let plan = d
        .dry_run("app", "SELECT * FROM signups WHERE channel = 'ads'")
        .await
        .unwrap();
    assert!(plan.to_lowercase().contains("signups"), "{plan}");
    assert!(matches!(
        d.execute("app", "DELETE FROM signups").await,
        Err(DataError::Refused(_))
    ));
    assert!(matches!(
        d.query("nope", "SELECT 1", None).await,
        Err(DataError::NotFound(_))
    ));
}

#[tokio::test]
async fn writable_connections_and_specs() {
    let dir = tempfile::tempdir().unwrap();
    let d = data(dir.path(), false).await;
    assert_eq!(
        d.execute("app", "DELETE FROM signups WHERE channel = 'referral'")
            .await
            .unwrap(),
        42
    );
    // Queries stay read-only even here.
    assert!(
        d.query("app", "UPDATE signups SET plan = 'x'", None)
            .await
            .is_err()
    );

    assert!(
        Data::parse(json!({"name": "x", "kind": "postgres", "url": "postgres://u:secret@h/db"}))
            .is_err()
    );
    assert!(Data::parse(json!({"name": "bad name", "kind": "sqlite", "path": "a"})).is_err());
    assert!(Data::parse(json!({"name": "x", "kind": "oracle"})).is_err());
    let p = Data::parse(
        json!({"name": "x", "kind": "postgres", "url": "postgres://u@h/db", "password_env": "PW"}),
    )
    .unwrap();
    assert!(p.read_only && p.max_rows == 1000);

    // Confined to an organization's folder.
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let confined = Data::confined(store, dir.path().to_path_buf());
    let outside =
        Data::parse(json!({"name": "etc", "kind": "sqlite", "path": "/etc/x.db"})).unwrap();
    assert!(confined.save(&outside).await.is_err());
    let lan =
        Data::parse(json!({"name": "lan", "kind": "postgres", "url": "postgres://u@10.0.0.5/db"}))
            .unwrap();
    assert!(confined.save(&lan).await.is_err());
    let inside = Data::parse(json!({"name": "app", "kind": "sqlite", "path": "app.db"})).unwrap();
    confined.save(&inside).await.unwrap();
    assert_eq!(
        confined
            .query("app", "SELECT COUNT(*) AS n FROM signups", None)
            .await
            .unwrap()
            .rows[0][0],
        json!(203)
    );
}

#[tokio::test]
async fn the_tools() {
    let dir = tempfile::tempdir().unwrap();
    let d = data(dir.path(), true).await;
    let tools = gai_data::tools::tools(d);
    let ws = tempfile::tempdir().unwrap();
    let ctx = ToolCtx::for_workspace(ws.path());
    let call = |name: &str, args: serde_json::Value| {
        let t = tools
            .iter()
            .find(|t| t.spec().name == name)
            .unwrap()
            .clone();
        let ctx = ctx.clone();
        async move { t.call(&ctx, args).await }
    };
    let text = |o: gai_tools::ToolOutput| {
        o.content
            .iter()
            .filter_map(|b| b.as_text())
            .collect::<String>()
    };

    let listed = text(call("sql_schema", json!({})).await.unwrap());
    assert!(
        listed.contains("app (sqlite, read-only): Signups by day and channel"),
        "{listed}"
    );
    let tables = text(
        call(
            "sql_schema",
            json!({"connection": "app", "tables": ["signups"]}),
        )
        .await
        .unwrap(),
    );
    assert!(
        tables.contains("signups") && !tables.contains("daily"),
        "{tables}"
    );

    let out = text(
        call(
            "sql_query",
            json!({"connection": "app", "sql": "SELECT day, COUNT(*) AS n FROM signups GROUP BY day ORDER BY day", "save_as": "daily signups"}),
        )
        .await
        .unwrap(),
    );
    assert!(
        out.starts_with("14 rows") && out.contains("Saved as data/daily-signups.csv"),
        "{out}"
    );
    let csv = std::fs::read_to_string(ws.path().join("data/daily-signups.csv")).unwrap();
    assert_eq!(csv.lines().count(), 15);
    let plan = text(
        call(
            "sql_query",
            json!({"connection": "app", "sql": "SELECT * FROM signups", "dry_run": true}),
        )
        .await
        .unwrap(),
    );
    assert!(plan.starts_with("The query is valid"), "{plan}");
    let e = call(
        "sql_query",
        json!({"connection": "app", "sql": "DELETE FROM signups"}),
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("only queries"), "{e}");
    let exec = tools
        .iter()
        .find(|t| t.spec().name == "sql_execute")
        .unwrap();
    assert_eq!(exec.default_policy(), gai_tools::ToolPolicy::Approve);
}
