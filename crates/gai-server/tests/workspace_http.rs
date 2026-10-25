//! The shared workspace over HTTP: knowledge only some may find,
//! conversation search, per-agent analytics, and who changed what.

use std::sync::Arc;

use gai::store::{DEFAULT_ORG, NewKey, Role, Scope, SharedStore, keys};
use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use reqwest::Method;
use serde_json::{Value, json};

struct Server {
    url: String,
    control: SharedStore,
    dir: tempfile::TempDir,
}

impl Server {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            data_dir: Some(dir.path().to_path_buf()),
            default_model: Some("mock/echo".into()),
            ..Default::default()
        };
        let gai = Arc::new(Gai::from_config(config, None).unwrap());
        let control = gai.runtime().await.unwrap().store().clone();
        let app = router(gai, AuthPolicy::new(AuthMode::Required, true, 0), None)
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { url, control, dir }
    }

    async fn admin_key(&self) -> String {
        let k = keys::generate();
        self.control
            .create_key(&NewKey::new(
                "admin",
                &k.prefix,
                &k.hash,
                &[Scope::Admin],
                None,
            ))
            .await
            .unwrap();
        k.secret
    }

    async fn user(&self, email: &str, role: Role) -> (String, String) {
        let u = self.control.create_user(email, None).await.unwrap();
        self.control
            .set_membership(DEFAULT_ORG, &u.id, role)
            .await
            .unwrap();
        let token = format!("gais_{}", &keys::generate().secret[4..]);
        self.control
            .create_login(
                &keys::hash(&token),
                &u.id,
                chrono::Utc::now() + chrono::Duration::hours(1),
            )
            .await
            .unwrap();
        (token, u.id)
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        key: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut rb = reqwest::Client::new()
            .request(method, format!("{}{path}", self.url))
            .bearer_auth(key);
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let r = rb.send().await.unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn knowledge_with_an_acl_is_found_only_by_who_it_names() {
    let s = Server::start().await;
    let admin = s.admin_key().await;
    let (eng_user, eng_id) = s.user("bea@example.com", Role::Member).await;
    let (other, _) = s.user("ola@example.com", Role::Member).await;
    let (_, team) = s
        .call(
            Method::POST,
            "/v1/teams",
            &admin,
            Some(json!({"name": "eng"})),
        )
        .await;
    s.control
        .add_team_member(team["id"].as_str().unwrap(), &eng_id)
        .await
        .unwrap();

    let docs = s.dir.path().join("handbook");
    std::fs::create_dir(&docs).unwrap();
    std::fs::write(
        docs.join("deploy.md"),
        "# Deploying\n\nThe launch code for production deploys is zebra-42.",
    )
    .unwrap();
    let secret = s.dir.path().join("public");
    std::fs::create_dir(&secret).unwrap();
    std::fs::write(
        secret.join("hello.md"),
        "# Welcome\n\nThe office opens at nine, launch is at noon.",
    )
    .unwrap();
    for (name, path, acl) in [
        ("eng-docs", &docs, json!(["team:eng"])),
        ("open-docs", &secret, json!([])),
    ] {
        let (st, err) = s
            .call(
                Method::POST,
                "/v1/sources",
                &admin,
                Some(json!({"name": name, "collection": "kb", "kind": "folder", "path": path, "acl": acl})),
            )
            .await;
        assert_eq!(st, 201, "{err}");
        let (st, stats) = s
            .call(
                Method::POST,
                &format!("/v1/sources/{name}/sync?wait=true"),
                &admin,
                None,
            )
            .await;
        assert_eq!(st, 200, "{stats}");
    }
    let (st, _) = s
        .call(
            Method::POST,
            "/v1/sources",
            &admin,
            Some(json!({"name": "bad", "collection": "kb", "kind": "folder", "path": "x", "acl": ["eng"]})),
        )
        .await;
    assert_eq!(st, 400);

    let search = |key: String| {
        let s = &s;
        async move {
            let (st, hits) = s
                .call(
                    Method::POST,
                    "/v1/collections/kb/search",
                    &key,
                    Some(json!({"query": "launch code", "mode": "text"})),
                )
                .await;
            assert_eq!(st, 200, "{hits}");
            hits.as_array()
                .unwrap()
                .iter()
                .map(|h| {
                    h["source"]
                        .as_str()
                        .unwrap()
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap()
                        .to_owned()
                })
                .collect::<Vec<_>>()
        }
    };
    let seen = search(admin.clone()).await;
    assert!(seen.contains(&"deploy.md".to_owned()), "{seen:?}");
    let seen = search(eng_user.clone()).await;
    assert!(seen.contains(&"deploy.md".to_owned()), "{seen:?}");
    let seen = search(other.clone()).await;
    assert!(!seen.contains(&"deploy.md".to_owned()), "{seen:?}");
    assert!(seen.contains(&"hello.md".to_owned()), "{seen:?}");

    // Listing and opening documents follow the same rule.
    let (_, listed) = s
        .call(Method::GET, "/v1/collections/kb/documents", &other, None)
        .await;
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    let (_, all) = s
        .call(Method::GET, "/v1/collections/kb/documents", &admin, None)
        .await;
    assert_eq!(all.as_array().unwrap().len(), 2);
    let hidden = all
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["source"].as_str().unwrap().ends_with("deploy.md"))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        s.call(
            Method::GET,
            &format!("/v1/collections/kb/documents/{hidden}"),
            &other,
            None
        )
        .await
        .0,
        404
    );
    assert_eq!(
        s.call(
            Method::GET,
            &format!("/v1/collections/kb/documents/{hidden}"),
            &eng_user,
            None
        )
        .await
        .0,
        200
    );

    // Opening it to everyone re-applies the acl at the next sync.
    s.call(
        Method::POST,
        "/v1/sources",
        &admin,
        Some(json!({"name": "eng-docs", "collection": "kb", "kind": "folder", "path": docs, "acl": []})),
    )
    .await;
    s.call(
        Method::POST,
        "/v1/sources/eng-docs/sync?wait=true",
        &admin,
        None,
    )
    .await;
    let seen = search(other).await;
    assert!(seen.contains(&"deploy.md".to_owned()), "{seen:?}");
}

#[tokio::test]
async fn conversations_analytics_and_changes() {
    let s = Server::start().await;
    let admin = s.admin_key().await;
    let (member, _) = s.user("mo@example.com", Role::Member).await;
    let (st, _) = s
        .call(
            Method::POST,
            "/v1/agents",
            &admin,
            Some(json!({"name": "helper", "model": "mock/echo"})),
        )
        .await;
    assert_eq!(st, 201);
    for text in ["Where is the quarterly report?", "Book a meeting room"] {
        let (st, run) = s
            .call(
                Method::POST,
                "/v1/runs",
                &member,
                Some(json!({"agent": "helper", "input": text})),
            )
            .await;
        assert_eq!(st, 200, "{run}");
    }
    let (st, hits) = s
        .call(
            Method::GET,
            "/v1/sessions/search?q=quarterly%20rep",
            &member,
            None,
        )
        .await;
    assert_eq!(st, 200, "{hits}");
    let hits = hits.as_array().unwrap();
    // The question, and mock/echo's answer repeating it.
    assert!(!hits.is_empty() && hits.len() <= 2, "{hits:?}");
    assert!(
        hits[0]["snippet"].as_str().unwrap().contains("[quarterly]"),
        "{hits:?}"
    );
    assert_eq!(hits[0]["agent"], "helper");
    let (_, none) = s
        .call(
            Method::GET,
            "/v1/sessions/search?q=quarterly&agent=other",
            &member,
            None,
        )
        .await;
    assert_eq!(none, json!([]));

    let (st, stats) = s
        .call(Method::GET, "/v1/analytics/agents", &member, None)
        .await;
    assert_eq!(st, 200, "{stats}");
    let helper = stats
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agent"] == "helper")
        .unwrap();
    assert_eq!(helper["runs"], 2);
    assert_eq!(helper["completed"], 2);
    assert_eq!(helper["success_rate"], 1.0);

    // Everyone sees who changed an agent; the full audit log is for admins.
    let (st, changes) = s
        .call(
            Method::GET,
            "/v1/changes?target=agents/helper",
            &member,
            None,
        )
        .await;
    assert_eq!(st, 200, "{changes}");
    assert_eq!(changes[0]["actor"], "admin");
    assert_eq!(changes[0]["action"], "POST /v1/agents");
    assert_eq!(s.call(Method::GET, "/v1/audit", &member, None).await.0, 403);
}
