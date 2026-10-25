//! The conformance suite on Postgres, when `GAI_TEST_POSTGRES_URL` is set
//! (`scripts/postgres-test.sh` starts one). Each test gets a schema of its
//! own.
//!
//!     GAI_TEST_POSTGRES_URL=... cargo test -p gai-store --features postgres --test postgres

#![cfg(feature = "postgres")]

use std::str::FromStr;
use std::sync::Arc;

use gai_store::{PostgresStore, SharedStore};
use sqlx::postgres::PgConnectOptions;

/// A store in a fresh schema, or `None` without a test database.
async fn store() -> Option<SharedStore> {
    let Ok(url) = std::env::var("GAI_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set GAI_TEST_POSTGRES_URL to run the Postgres tests");
        return None;
    };
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = gai_core::new_id("t").replace('-', "_");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let opts = PgConnectOptions::from_str(&url)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    Some(Arc::new(PostgresStore::connect_with(opts).await.unwrap()) as SharedStore)
}

gai_store::conformance_tests!(store());

#[tokio::test]
async fn migrations_run_twice() {
    let Ok(url) = std::env::var("GAI_TEST_POSTGRES_URL") else {
        return;
    };
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = gai_core::new_id("t");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let opts = || {
        PgConnectOptions::from_str(&url)
            .unwrap()
            .options([("search_path", schema.as_str())])
    };
    PostgresStore::connect_with(opts()).await.unwrap();
    PostgresStore::connect_with(opts()).await.unwrap();
}

/// Organizations' schemas don't see each other, even after the main one
/// (in `public`) exists.
#[tokio::test]
async fn organization_schemas_are_apart() {
    let Ok(url) = std::env::var("GAI_TEST_POSTGRES_URL") else {
        return;
    };
    use gai_store::AgentStore;
    let a = gai_core::new_id("org_a").replace('-', "_");
    let b = gai_core::new_id("org_b").replace('-', "_");
    let sa = PostgresStore::connect_schema(&url, &a).await.unwrap();
    let sb = PostgresStore::connect_schema(&url, &b).await.unwrap();
    sa.put_agent("only-a", &serde_json::json!({"model": "x"}), None)
        .await
        .unwrap();
    assert!(sb.get_agent("only-a", None).await.unwrap().is_none());
    assert!(sa.get_agent("only-a", None).await.unwrap().is_some());
    // Reopening keeps the data: the migrations ran in the schema itself.
    let again = PostgresStore::connect_schema(&url, &a).await.unwrap();
    assert!(again.get_agent("only-a", None).await.unwrap().is_some());
    assert!(PostgresStore::connect_schema(&url, "bad;name").await.is_err());
}
