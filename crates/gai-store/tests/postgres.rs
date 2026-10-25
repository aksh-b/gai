//! The Postgres store against a real server, when `GAI_TEST_POSTGRES_URL`
//! is set (e.g. `postgres://postgres:gai@localhost:5432/postgres`). Each
//! test works in a schema of its own, dropped afterwards.
//!
//!     GAI_TEST_POSTGRES_URL=... cargo test -p gai-store --features postgres --test postgres

#![cfg(feature = "postgres")]

use std::str::FromStr;

use gai_store::PostgresStore;
use gai_store::conformance;
use sqlx::postgres::PgConnectOptions;

/// A store in a fresh schema, and that schema's name.
async fn store() -> Option<(PostgresStore, String, sqlx::PgPool)> {
    let Ok(url) = std::env::var("GAI_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set GAI_TEST_POSTGRES_URL to run the Postgres tests");
        return None;
    };
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!(
        "gai_test_{}",
        gai_core::new_id("s").to_lowercase().replace('-', "_")
    );
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let opts = PgConnectOptions::from_str(&url)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    Some((
        PostgresStore::connect_with(opts).await.unwrap(),
        schema,
        admin,
    ))
}

async fn drop_schema(schema: &str, admin: &sqlx::PgPool) {
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(admin)
        .await
        .unwrap();
}

#[tokio::test]
async fn agents_conform() {
    let Some((s, schema, admin)) = store().await else {
        return;
    };
    conformance::agents(&s).await;
    drop_schema(&schema, &admin).await;
}

#[tokio::test]
async fn settings_conform() {
    let Some((s, schema, admin)) = store().await else {
        return;
    };
    conformance::settings(&s).await;
    drop_schema(&schema, &admin).await;
}

#[tokio::test]
async fn mcp_conforms() {
    let Some((s, schema, admin)) = store().await else {
        return;
    };
    conformance::mcp(&s).await;
    // Migrations are idempotent: connecting again is fine.
    let url = std::env::var("GAI_TEST_POSTGRES_URL").unwrap();
    let opts = PgConnectOptions::from_str(&url)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    PostgresStore::connect_with(opts).await.unwrap();
    drop_schema(&schema, &admin).await;
}
