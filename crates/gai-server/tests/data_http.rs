//! Database connections over HTTP: admins save them, members query them.

use std::sync::Arc;

use gai::store::{NewKey, Scope, keys};
use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use reqwest::Method;
use serde_json::{Value, json};

#[tokio::test]
async fn connections_and_queries() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("shop.db");
    {
        let opts = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&db)
            .create_if_missing(true);
        let pool = sqlx::SqlitePool::connect_with(opts).await.unwrap();
        sqlx::query("CREATE TABLE orders (id INTEGER PRIMARY KEY, total REAL, status TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO orders VALUES (1, 9.5, 'paid'), (2, 20, 'paid'), (3, 4, 'refunded')",
        )
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
    let store = gai.runtime().await.unwrap().store().clone();
    let key = |scopes: &[Scope]| {
        let store = store.clone();
        let scopes = scopes.to_vec();
        async move {
            let k = keys::generate();
            store
                .create_key(&NewKey::new("k", &k.prefix, &k.hash, &scopes, None))
                .await
                .unwrap();
            k.secret
        }
    };
    let (admin, member) = (key(&[Scope::Admin]).await, key(&[Scope::Run]).await);
    let app = router(gai, AuthPolicy::new(AuthMode::Required, true, 0), None)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let call = |m: Method, path: &str, k: &str, body: Option<Value>| {
        let mut rb = reqwest::Client::new()
            .request(m, format!("{url}{path}"))
            .bearer_auth(k);
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        async move {
            let r = rb.send().await.unwrap();
            (
                r.status().as_u16(),
                r.json::<Value>().await.unwrap_or(Value::Null),
            )
        }
    };

    let yaml = format!(
        "name: shop\nkind: sqlite\npath: {}\ndescription: Orders\n",
        db.display()
    );
    assert_eq!(
        call(
            Method::POST,
            "/v1/connections",
            &member,
            Some(json!({"yaml": yaml}))
        )
        .await
        .0,
        403
    );
    let (st, c) = call(
        Method::POST,
        "/v1/connections",
        &admin,
        Some(json!({"yaml": yaml})),
    )
    .await;
    assert_eq!(st, 201, "{c}");
    assert_eq!(c["schema"]["tables"][0]["name"], "orders");
    let (st, _) = call(
        Method::POST,
        "/v1/connections",
        &admin,
        Some(json!({"name": "gone", "kind": "sqlite", "path": dir.path().join("missing.db")})),
    )
    .await;
    assert_eq!(st, 400, "a connection that doesn't answer is reported");

    let (st, r) = call(
        Method::POST,
        "/v1/connections/shop/query",
        &member,
        Some(json!({"sql": "SELECT status, SUM(total) AS total FROM orders GROUP BY status ORDER BY status"})),
    )
    .await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["rows"], json!([["paid", 29.5], ["refunded", 4.0]]));
    assert_eq!(r["columns"][1]["name"], "total");
    let (st, r) = call(
        Method::POST,
        "/v1/connections/shop/query",
        &member,
        Some(json!({"sql": "SELECT * FROM orders", "dry_run": true})),
    )
    .await;
    assert_eq!(st, 200);
    assert!(r["plan"].as_str().unwrap().contains("orders"), "{r}");
    let (st, e) = call(
        Method::POST,
        "/v1/connections/shop/query",
        &member,
        Some(json!({"sql": "DELETE FROM orders"})),
    )
    .await;
    assert_eq!(st, 403, "{e}");
    let (st, s) = call(
        Method::GET,
        "/v1/connections/shop/schema?refresh=true",
        &member,
        None,
    )
    .await;
    assert_eq!((st, s["tables"][0]["rows"].clone()), (200, json!(3)));
    let (_, tools) = call(Method::GET, "/v1/tools", &member, None).await;
    let names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["spec"]["name"].as_str())
        .collect();
    assert!(
        names.contains(&"sql_query")
            && names.contains(&"sql_schema")
            && names.contains(&"sql_execute")
    );
    assert_eq!(
        call(Method::DELETE, "/v1/connections/shop", &member, None)
            .await
            .0,
        403
    );
    assert_eq!(
        call(Method::DELETE, "/v1/connections/shop", &admin, None)
            .await
            .0,
        200
    );
    assert_eq!(
        call(Method::GET, "/v1/connections/shop", &member, None)
            .await
            .0,
        404
    );
}
