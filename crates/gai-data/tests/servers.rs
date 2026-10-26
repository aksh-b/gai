//! Postgres and MySQL connections, when `GAI_TEST_PG` / `GAI_TEST_MYSQL`
//! (and `GAI_TEST_DB_PASSWORD`) are set: `scripts/data-test.sh` starts both.

use std::sync::Arc;

use gai_data::{Data, DataError};
use gai_store::{SharedStore, SqliteStore};
use serde_json::json;

async fn fill(kind: &str, url: &str) {
    let pw = std::env::var("GAI_TEST_DB_PASSWORD").unwrap();
    let stmts = [
        "DROP TABLE IF EXISTS signups",
        "CREATE TABLE signups (id INTEGER PRIMARY KEY, day DATE NOT NULL, channel VARCHAR(20) NOT NULL, amount NUMERIC(10,2))",
    ];
    let mut rows = Vec::new();
    let mut id = 0;
    for day in 1..=14 {
        for (channel, n) in [("organic", 4), ("ads", if day <= 7 { 3 } else { 0 })] {
            for _ in 0..n {
                id += 1;
                rows.push(format!(
                    "({id}, '2026-10-{day:02}', '{channel}', {}.50)",
                    id % 7
                ));
            }
        }
    }
    let insert = format!(
        "INSERT INTO signups (id, day, channel, amount) VALUES {}",
        rows.join(", ")
    );
    match kind {
        "postgres" => {
            let opts: sqlx::postgres::PgConnectOptions = url
                .parse::<sqlx::postgres::PgConnectOptions>()
                .unwrap()
                .password(&pw);
            let pool = sqlx::PgPool::connect_with(opts).await.unwrap();
            for s in stmts.iter().map(|s| s.to_string()).chain([insert]) {
                sqlx::query(&s).execute(&pool).await.unwrap();
            }
        }
        _ => {
            let opts: sqlx::mysql::MySqlConnectOptions = url
                .parse::<sqlx::mysql::MySqlConnectOptions>()
                .unwrap()
                .password(&pw);
            let pool = sqlx::MySqlPool::connect_with(opts).await.unwrap();
            for s in stmts.iter().map(|s| s.to_string()).chain([insert]) {
                sqlx::query(&s).execute(&pool).await.unwrap();
            }
        }
    }
}

async fn check(kind: &str, env: &str) {
    let Ok(url) = std::env::var(env) else {
        eprintln!("skipped: set {env} (scripts/data-test.sh)");
        return;
    };
    fill(kind, &url).await;
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let d = Data::new(store);
    d.save(&Data::parse(json!({"name": "app", "kind": kind, "url": url, "password_env": "GAI_TEST_DB_PASSWORD", "max_rows": 50})).unwrap())
        .await
        .unwrap();

    let s = d.schema("app", true).await.unwrap();
    let t = s
        .tables
        .iter()
        .find(|t| t.name == "signups")
        .unwrap_or_else(|| panic!("{s:?}"));
    assert_eq!(
        t.columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "day", "channel", "amount"]
    );
    assert!(!t.columns[1].nullable && t.columns[3].nullable);

    let r = d
        .query(
            "app",
            "SELECT channel, COUNT(*) AS n, MIN(day) AS first_day, SUM(amount) AS total FROM signups
             WHERE day >= '2026-10-08' GROUP BY channel ORDER BY channel",
            None,
        )
        .await
        .unwrap();
    assert_eq!(r.rows.len(), 1, "ads stopped in the second week: {r:?}");
    assert_eq!(r.rows[0][0], json!("organic"));
    assert_eq!(r.rows[0][1], json!(28));
    assert_eq!(r.rows[0][2], json!("2026-10-08"));
    assert!(
        r.rows[0][3].as_f64().is_some() || r.rows[0][3].is_string(),
        "{:?}",
        r.rows[0][3]
    );
    let r = d
        .query("app", "SELECT * FROM signups ORDER BY id", None)
        .await
        .unwrap();
    assert_eq!((r.rows.len(), r.truncated), (50, true));

    assert!(matches!(
        d.query("app", "DELETE FROM signups", None).await,
        Err(DataError::Refused(_))
    ));
    // A write the parser lets through still fails in the read-only transaction.
    if kind == "postgres" {
        let r = d
            .query(
                "app",
                "WITH gone AS (DELETE FROM signups RETURNING id) SELECT COUNT(*) FROM gone",
                None,
            )
            .await;
        assert!(r.is_err(), "{r:?}");
    }
    let n = d
        .query("app", "SELECT COUNT(*) AS n FROM signups", None)
        .await
        .unwrap();
    assert_eq!(n.rows[0][0], json!(77));
    assert!(
        !d.dry_run("app", "SELECT * FROM signups WHERE channel = 'ads'")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        d.execute("app", "DELETE FROM signups").await,
        Err(DataError::Refused(_))
    ));

    d.save(&Data::parse(json!({"name": "app", "kind": kind, "url": url, "password_env": "GAI_TEST_DB_PASSWORD", "read_only": false})).unwrap())
        .await
        .unwrap();
    assert_eq!(
        d.execute("app", "DELETE FROM signups WHERE channel = 'ads'")
            .await
            .unwrap(),
        21
    );
}

#[tokio::test]
async fn postgres() {
    check("postgres", "GAI_TEST_PG").await;
}

#[tokio::test]
async fn mysql() {
    check("mysql", "GAI_TEST_MYSQL").await;
}
