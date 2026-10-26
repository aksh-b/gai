//! BigQuery and Snowflake connections against mocks of their REST APIs.
//! `test-key.pem` is a throwaway key made for these tests.

use std::sync::Arc;

use gai_data::{Data, DataError};
use gai_store::{SharedStore, SqliteStore};
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, header, method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

async fn data() -> Data {
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    Data::new(store)
}

/// BigQuery's answer to a dry run or a query.
struct Bq;

impl Respond for Bq {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let sql = body["query"].as_str().unwrap_or_default();
        if body["dryRun"] == true {
            let kind = if sql.to_uppercase().starts_with("SELECT") {
                "SELECT"
            } else {
                "DELETE"
            };
            let bytes = if sql.contains("huge") {
                "99000000000"
            } else {
                "1200000"
            };
            return ResponseTemplate::new(200).set_body_json(json!({"statistics": {"query": {"statementType": kind}}, "totalBytesProcessed": bytes, "jobComplete": true}));
        }
        if sql.contains("INFORMATION_SCHEMA") {
            return ResponseTemplate::new(200).set_body_json(json!({
                "jobComplete": true,
                "schema": {"fields": [{"name": "table_name"}, {"name": "column_name"}, {"name": "data_type"}, {"name": "is_nullable"}, {"name": "table_type"}]},
                "rows": [
                    {"f": [{"v": "signups"}, {"v": "day"}, {"v": "DATE"}, {"v": "NO"}, {"v": "BASE TABLE"}]},
                    {"f": [{"v": "signups"}, {"v": "channel"}, {"v": "STRING"}, {"v": "YES"}, {"v": "BASE TABLE"}]}
                ]
            }));
        }
        // The first answer isn't complete; asking again gets the rows.
        ResponseTemplate::new(200).set_body_json(
            json!({"jobComplete": false, "jobReference": {"jobId": "job1", "location": "EU"}}),
        )
    }
}

#[tokio::test]
async fn bigquery() {
    let api = MockServer::start().await;
    let key = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/test-key.pem"))
        .unwrap();
    // SAFETY: tests in this file don't read this variable concurrently.
    unsafe {
        std::env::set_var(
            "GAI_TEST_BQ_KEY",
            json!({"client_email": "gai@proj.iam.gserviceaccount.com", "private_key": key, "token_uri": format!("{}/token", api.uri())}).to_string(),
        );
    }
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": "ya29.test", "expires_in": 3600})),
        )
        .expect(1)
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path("/bigquery/v2/projects/proj/queries"))
        .and(header("authorization", "Bearer ya29.test"))
        .respond_with(Bq)
        .mount(&api)
        .await;
    Mock::given(method("GET"))
        .and(path("/bigquery/v2/projects/proj/queries/job1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jobComplete": true,
            "totalRows": "3",
            "schema": {"fields": [{"name": "channel", "type": "STRING"}, {"name": "n", "type": "INTEGER"}, {"name": "share", "type": "FLOAT"}, {"name": "at", "type": "TIMESTAMP"}]},
            "rows": [
                {"f": [{"v": "organic"}, {"v": "28"}, {"v": "0.8"}, {"v": "1.7613792E9"}]},
                {"f": [{"v": "ads"}, {"v": "7"}, {"v": "0.2"}, {"v": null}]},
                {"f": [{"v": "referral"}, {"v": "1"}, {"v": "0.0"}, {"v": null}]}
            ]
        })))
        .mount(&api)
        .await;
    let d = data().await;
    d.save(
        &Data::parse(json!({
            "name": "bq", "kind": "bigquery", "project": "proj", "dataset": "app",
            "credentials_env": "GAI_TEST_BQ_KEY", "api_url": api.uri(), "max_rows": 2,
        }))
        .unwrap(),
    )
    .await
    .unwrap();

    let s = d.schema("bq", true).await.unwrap();
    assert_eq!(s.tables[0].schema.as_deref(), Some("app"));
    assert_eq!(s.tables[0].columns.len(), 2);
    let r = d
        .query(
            "bq",
            "SELECT channel, COUNT(*) AS n FROM signups GROUP BY 1",
            None,
        )
        .await
        .unwrap();
    assert_eq!(r.rows.len(), 2);
    assert!(r.truncated);
    assert_eq!(r.rows[0][..3], [json!("organic"), json!(28), json!(0.8)]);
    assert!(
        r.rows[0][3].as_str().unwrap().starts_with("2025-"),
        "{:?}",
        r.rows[0][3]
    );
    let plan = d.dry_run("bq", "SELECT * FROM signups").await.unwrap();
    assert_eq!(plan, "SELECT: would scan 1.2 MB");
    let e = d.query("bq", "SELECT * FROM huge", None).await.unwrap_err();
    assert!(
        matches!(e, DataError::Refused(_)) && e.to_string().contains("scan"),
        "{e}"
    );
    assert!(matches!(
        d.query("bq", "DELETE FROM t WHERE true", None).await,
        Err(DataError::Refused(_))
    ));
}

#[tokio::test]
async fn snowflake() {
    let api = MockServer::start().await;
    // SAFETY: as above.
    unsafe { std::env::set_var("GAI_TEST_SF_TOKEN", "pat-123") };
    Mock::given(method("POST"))
        .and(path("/api/v2/statements"))
        .and(header("authorization", "Bearer pat-123"))
        .and(header(
            "x-snowflake-authorization-token-type",
            "PROGRAMMATIC_ACCESS_TOKEN",
        ))
        .and(body_partial_json(
            json!({"warehouse": "WH", "database": "APP"}),
        ))
        .respond_with(|req: &Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            if body["statement"]
                .as_str()
                .unwrap()
                .contains("information_schema")
            {
                return ResponseTemplate::new(200).set_body_json(json!({
                    "resultSetMetaData": {"numRows": 1, "rowType": []},
                    "data": [["PUBLIC", "SIGNUPS", "DAY", "DATE", "NO", "BASE TABLE", "245"]]
                }));
            }
            ResponseTemplate::new(202)
                .set_body_json(json!({"statementHandle": "h1", "message": "running"}))
        })
        .mount(&api)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("^/api/v2/statements/h1$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "resultSetMetaData": {"numRows": 2, "rowType": [
                {"name": "DAY", "type": "date"}, {"name": "N", "type": "fixed", "scale": 0},
                {"name": "RATE", "type": "fixed", "scale": 2}, {"name": "OK", "type": "boolean"}
            ]},
            "data": [["20734", "12", "0.25", "true"], ["20735", "3", null, "false"]]
        })))
        .mount(&api)
        .await;
    let d = data().await;
    d.save(
        &Data::parse(json!({
            "name": "sf", "kind": "snowflake", "account": "org-acct", "database": "APP", "warehouse": "WH",
            "token_env": "GAI_TEST_SF_TOKEN", "api_url": api.uri(),
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    let s = d.schema("sf", false).await.unwrap();
    assert_eq!(
        (s.tables[0].name.as_str(), s.tables[0].rows),
        ("SIGNUPS", Some(245))
    );
    let r = d
        .query(
            "sf",
            "SELECT day, COUNT(*) AS n FROM signups GROUP BY day",
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        r.rows[0],
        [json!("2026-10-08"), json!(12), json!(0.25), json!(true)]
    );
    assert_eq!(r.rows[1][2], json!(null));
    assert!(matches!(
        d.query("sf", "DELETE FROM signups", None).await,
        Err(DataError::Refused(_))
    ));
    // Snowflake has no read-only transactions: SQL the parser can't read is refused.
    assert!(d.query("sf", "SELECT FROM WHERE ;;", None).await.is_err());
}
