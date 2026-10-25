//! Two organizations on one server: each sees only its own data, roles
//! and teams decide who may change what, and other organizations are
//! confined.

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

    async fn key(&self, org: &str, scopes: &[Scope]) -> String {
        let k = keys::generate();
        let mut new = NewKey::new("k", &k.prefix, &k.hash, scopes, None);
        new.org_id = org.into();
        self.control.create_key(&new).await.unwrap();
        k.secret
    }

    /// A signed-in user's token with this role in the organization.
    async fn user(&self, email: &str, org: &str, role: Role) -> (String, String) {
        let u = self.control.create_user(email, None).await.unwrap();
        self.control.set_membership(org, &u.id, role).await.unwrap();
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
        org: Option<&str>,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut rb = reqwest::Client::new()
            .request(method, format!("{}{path}", self.url))
            .bearer_auth(key);
        if let Some(o) = org {
            rb = rb.header("x-gai-org", o);
        }
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let r = rb.send().await.unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }
}

fn names(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .filter_map(|a| {
            a["summary"]["name"]
                .as_str()
                .or(a["name"].as_str())
                .map(str::to_owned)
        })
        .collect()
}

#[tokio::test]
async fn organizations_dont_see_each_other() {
    let s = Server::start().await;
    let root = s.key(DEFAULT_ORG, &[Scope::Admin]).await;
    let (st, org) = s
        .call(
            Method::POST,
            "/v1/orgs",
            &root,
            None,
            Some(json!({"slug": "acme", "name": "Acme"})),
        )
        .await;
    assert_eq!(st, 201, "{org}");
    let acme_id = org["id"].as_str().unwrap().to_owned();
    let (st, _) = s
        .call(
            Method::POST,
            "/v1/orgs",
            &root,
            None,
            Some(json!({"slug": "acme", "name": "Again"})),
        )
        .await;
    assert_eq!(st, 409);
    assert!(s.dir.path().join("orgs/acme/gai.db").exists());

    let acme = s.key(&acme_id, &[Scope::Admin]).await;
    // Only server admins make organizations.
    let (st, _) = s
        .call(
            Method::POST,
            "/v1/orgs",
            &acme,
            None,
            Some(json!({"slug": "other", "name": "Other"})),
        )
        .await;
    assert_eq!(st, 403);

    let agent = |name: &str| json!({"name": name, "model": "mock/echo"});
    assert_eq!(
        s.call(
            Method::POST,
            "/v1/agents",
            &root,
            None,
            Some(agent("helper"))
        )
        .await
        .0,
        201
    );
    assert_eq!(
        s.call(Method::POST, "/v1/agents", &acme, None, Some(agent("bot")))
            .await
            .0,
        201
    );
    let (_, mine) = s.call(Method::GET, "/v1/agents", &acme, None, None).await;
    assert!(names(&mine).contains(&"bot".to_owned()), "{mine}");
    assert!(!names(&mine).contains(&"helper".to_owned()), "{mine}");
    let (_, theirs) = s.call(Method::GET, "/v1/agents", &root, None, None).await;
    assert!(!names(&theirs).contains(&"bot".to_owned()), "{theirs}");
    assert_eq!(
        s.call(Method::GET, "/v1/agents/helper", &acme, None, None)
            .await
            .0,
        404
    );

    // Runs and sessions too.
    let (st, run) = s
        .call(
            Method::POST,
            "/v1/runs",
            &acme,
            None,
            Some(json!({"input": "hello acme"})),
        )
        .await;
    assert_eq!(st, 200, "{run}");
    let run_id = run["run_id"]
        .as_str()
        .or(run["id"].as_str())
        .unwrap()
        .to_owned();
    assert_eq!(
        s.call(
            Method::GET,
            &format!("/v1/runs/{run_id}"),
            &acme,
            None,
            None
        )
        .await
        .0,
        200
    );
    assert_eq!(
        s.call(
            Method::GET,
            &format!("/v1/runs/{run_id}"),
            &root,
            None,
            None
        )
        .await
        .0,
        404
    );

    // A key can't be pointed at another organization.
    assert_eq!(
        s.call(Method::GET, "/v1/agents", &acme, Some("default"), None)
            .await
            .0,
        403
    );

    // Keys list per organization.
    let (st, created) = s
        .call(
            Method::POST,
            "/v1/keys",
            &acme,
            None,
            Some(json!({"name": "ci", "scopes": ["run"]})),
        )
        .await;
    assert_eq!(st, 201, "{created}");
    assert_eq!(created["key"]["org_id"], acme_id.as_str());
    let (_, acme_keys) = s.call(Method::GET, "/v1/keys", &acme, None, None).await;
    let (_, root_keys) = s.call(Method::GET, "/v1/keys", &root, None, None).await;
    assert_eq!(acme_keys.as_array().unwrap().len(), 2);
    assert!(
        root_keys
            .as_array()
            .unwrap()
            .iter()
            .all(|k| k["org_id"] == DEFAULT_ORG)
    );
    let root_key_id = root_keys[0]["id"].as_str().unwrap();
    assert_eq!(
        s.call(
            Method::DELETE,
            &format!("/v1/keys/{root_key_id}"),
            &acme,
            None,
            None
        )
        .await
        .0,
        404
    );

    // The audit log is per organization.
    let (_, audit) = s.call(Method::GET, "/v1/audit", &root, None, None).await;
    assert!(
        audit
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["target"] != "agents/bot"),
        "{audit}"
    );

    // /mcp serves the default organization only.
    assert_eq!(
        s.call(Method::POST, "/mcp", &acme, None, Some(json!({})))
            .await
            .0,
        403
    );
}

#[tokio::test]
async fn other_organizations_are_confined() {
    let s = Server::start().await;
    let root = s.key(DEFAULT_ORG, &[Scope::Admin]).await;
    let (_, org) = s
        .call(
            Method::POST,
            "/v1/orgs",
            &root,
            None,
            Some(json!({"slug": "acme", "name": "Acme"})),
        )
        .await;
    let acme = s.key(org["id"].as_str().unwrap(), &[Scope::Admin]).await;

    let (_, tools) = s.call(Method::GET, "/v1/tools", &acme, None, None).await;
    let tool_names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["spec"]["name"].as_str())
        .collect();
    assert!(!tool_names.contains(&"shell"), "{tool_names:?}");
    assert!(tool_names.contains(&"fs_read"));
    let (_, tools) = s.call(Method::GET, "/v1/tools", &root, None, None).await;
    assert!(
        tools
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["spec"]["name"] == "shell")
    );

    for config in [
        json!({"command": "sh", "args": ["-c", "id"]}),
        json!({"url": "http://127.0.0.1:9/mcp"}),
        json!({"url": "https://mcp.example.com/mcp", "headers": {"authorization": "Bearer ${OPENAI_API_KEY}"}}),
    ] {
        let (st, err) = s
            .call(
                Method::POST,
                "/v1/mcp",
                &acme,
                None,
                Some(json!({"name": "x", "config": config})),
            )
            .await;
        assert_eq!(st, 400, "{config} {err}");
    }

    let (st, err) = s
        .call(
            Method::POST,
            "/v1/sources",
            &acme,
            None,
            Some(json!({"name": "etc", "collection": "c", "kind": "folder", "path": "/etc"})),
        )
        .await;
    assert_eq!(st, 400, "{err}");
    let (st, _) = s
        .call(
            Method::POST,
            "/v1/sources",
            &acme,
            None,
            Some(json!({"name": "docs", "collection": "c", "kind": "folder", "path": "docs"})),
        )
        .await;
    assert_eq!(st, 201);
    let (st, _) = s
        .call(
            Method::POST,
            "/v1/sources",
            &acme,
            None,
            Some(json!({"name": "lan", "collection": "c", "kind": "web", "urls": ["http://10.0.0.1/"], "allow_private": true})),
        )
        .await;
    assert_eq!(st, 400);

    // Secrets: write-only values.
    assert_eq!(
        s.call(
            Method::PUT,
            "/v1/secrets/GITHUB_TOKEN",
            &acme,
            None,
            Some(json!({"value": "t0k"}))
        )
        .await
        .0,
        200
    );
    let (_, secrets) = s.call(Method::GET, "/v1/secrets", &acme, None, None).await;
    assert_eq!(secrets, json!(["GITHUB_TOKEN"]));
    let (_, secrets) = s.call(Method::GET, "/v1/secrets", &root, None, None).await;
    assert_eq!(secrets, json!([]));

    // Budgets are set by server admins and stop new work.
    let (st, _) = s
        .call(
            Method::PATCH,
            "/v1/orgs/acme",
            &acme,
            None,
            Some(json!({"settings": {"limits": {"budget_usd_per_month": 0.0}, "branding": {"product_name": "Acme AI"}}})),
        )
        .await;
    // An organization brands itself, but its limits are the server admins'.
    assert_eq!(st, 200);
    assert_eq!(
        s.call(
            Method::POST,
            "/v1/runs",
            &acme,
            None,
            Some(json!({"input": "hi"}))
        )
        .await
        .0,
        200
    );
    let (st, org) = s
        .call(
            Method::PATCH,
            "/v1/orgs/acme",
            &root,
            None,
            Some(json!({"settings": {"limits": {"budget_usd_per_month": 0.0}, "branding": {"product_name": "Acme AI"}}})),
        )
        .await;
    assert_eq!(st, 200, "{org}");
    assert_eq!(org["settings"]["branding"]["product_name"], "Acme AI");
    let (st, err) = s
        .call(
            Method::POST,
            "/v1/runs",
            &acme,
            None,
            Some(json!({"input": "hi"})),
        )
        .await;
    assert_eq!(st, 402, "{err}");
    assert_eq!(
        s.call(Method::GET, "/v1/agents", &acme, None, None).await.0,
        200
    );
}

#[tokio::test]
async fn roles_and_teams() {
    let s = Server::start().await;
    let root = s.key(DEFAULT_ORG, &[Scope::Admin]).await;
    let (_, org) = s
        .call(
            Method::POST,
            "/v1/orgs",
            &root,
            None,
            Some(json!({"slug": "acme", "name": "Acme", "owner_email": "ada@acme.test"})),
        )
        .await;
    let acme_id = org["id"].as_str().unwrap().to_owned();
    let ada = s
        .control
        .find_user_by_email("ada@acme.test")
        .await
        .unwrap()
        .unwrap();
    let ada_token = format!("gais_{}", &keys::generate().secret[4..]);
    s.control
        .create_login(
            &keys::hash(&ada_token),
            &ada.id,
            chrono::Utc::now() + chrono::Duration::hours(1),
        )
        .await
        .unwrap();
    let (viewer, _) = s.user("vic@acme.test", &acme_id, Role::Viewer).await;
    let (member, _) = s.user("mo@acme.test", &acme_id, Role::Member).await;
    let (builder, _) = s.user("bea@acme.test", &acme_id, Role::Builder).await;
    let (other, _) = s.user("ola@acme.test", &acme_id, Role::Builder).await;

    let (st, me) = s.call(Method::GET, "/v1/me", &ada_token, None, None).await;
    assert_eq!(st, 200, "{me}");
    assert_eq!(me["org"]["slug"], "acme");
    assert_eq!(me["principal"]["role"], "owner");
    assert_eq!(me["memberships"].as_array().unwrap().len(), 1);
    // Not a member of the default organization.
    assert_eq!(
        s.call(Method::GET, "/v1/agents", &ada_token, Some("default"), None)
            .await
            .0,
        403
    );

    let agent = |name: &str| json!({"name": name, "model": "mock/echo"});
    assert_eq!(
        s.call(Method::POST, "/v1/agents", &viewer, None, Some(agent("a")))
            .await
            .0,
        403
    );
    assert_eq!(
        s.call(Method::POST, "/v1/agents", &member, None, Some(agent("a")))
            .await
            .0,
        403
    );
    assert_eq!(
        s.call(
            Method::POST,
            "/v1/runs",
            &member,
            None,
            Some(json!({"input": "hi"}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        s.call(
            Method::POST,
            "/v1/runs",
            &viewer,
            None,
            Some(json!({"input": "hi"}))
        )
        .await
        .0,
        403
    );
    assert_eq!(
        s.call(
            Method::POST,
            "/v1/agents",
            &builder,
            None,
            Some(agent("helper"))
        )
        .await
        .0,
        201
    );
    assert_eq!(
        s.call(Method::GET, "/v1/keys", &builder, None, None)
            .await
            .0,
        403
    );

    // A team owns the helper; only its builders change it.
    let (st, team) = s
        .call(
            Method::POST,
            "/v1/teams",
            &ada_token,
            None,
            Some(json!({"name": "eng"})),
        )
        .await;
    assert_eq!(st, 201, "{team}");
    assert_eq!(
        s.call(
            Method::PUT,
            "/v1/teams/eng/members/bea@acme.test",
            &ada_token,
            None,
            None
        )
        .await
        .0,
        200
    );
    assert_eq!(
        s.call(
            Method::PUT,
            "/v1/teams/eng/members/nobody@acme.test",
            &ada_token,
            None,
            None
        )
        .await
        .0,
        400
    );
    let (st, owned) = s
        .call(
            Method::PUT,
            "/v1/owners/agent/helper",
            &builder,
            None,
            Some(json!({"team": "eng"})),
        )
        .await;
    assert_eq!(st, 200, "{owned}");
    assert_eq!(
        s.call(
            Method::POST,
            "/v1/agents",
            &other,
            None,
            Some(agent("helper"))
        )
        .await
        .0,
        403
    );
    assert_eq!(
        s.call(Method::DELETE, "/v1/agents/helper", &other, None, None)
            .await
            .0,
        403
    );
    assert_eq!(
        s.call(
            Method::POST,
            "/v1/agents",
            &other,
            None,
            Some(json!({"yaml": "name: helper\nmodel: mock/echo\n"}))
        )
        .await
        .0,
        403
    );
    assert_eq!(
        s.call(
            Method::POST,
            "/v1/agents",
            &builder,
            None,
            Some(agent("helper"))
        )
        .await
        .0,
        201
    );
    let (_, owners) = s.call(Method::GET, "/v1/owners", &viewer, None, None).await;
    assert_eq!(owners[0]["team"], "eng");

    // Members: invite, change, and keep an owner.
    let (st, m) = s
        .call(
            Method::PUT,
            "/v1/members/new@acme.test",
            &ada_token,
            None,
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(st, 200, "{m}");
    assert_eq!(m["role"], "member");
    let (_, members) = s
        .call(Method::GET, "/v1/members", &member, None, None)
        .await;
    assert_eq!(members.as_array().unwrap().len(), 6);
    assert_eq!(
        s.call(
            Method::PUT,
            "/v1/members/ada@acme.test",
            &ada_token,
            None,
            Some(json!({"role": "admin"}))
        )
        .await
        .0,
        409
    );
    assert_eq!(
        s.call(
            Method::DELETE,
            "/v1/members/ada@acme.test",
            &ada_token,
            None,
            None
        )
        .await
        .0,
        409
    );
    // Admins can't make owners.
    let (admin, _) = s.user("al@acme.test", &acme_id, Role::Admin).await;
    assert_eq!(
        s.call(
            Method::PUT,
            "/v1/members/mo@acme.test",
            &admin,
            None,
            Some(json!({"role": "owner"}))
        )
        .await
        .0,
        403
    );
    assert_eq!(
        s.call(
            Method::PUT,
            "/v1/members/mo@acme.test",
            &admin,
            None,
            Some(json!({"role": "builder"}))
        )
        .await
        .0,
        200
    );

    // A team key is limited like the team.
    let (st, k) = s
        .call(
            Method::POST,
            "/v1/keys",
            &admin,
            None,
            Some(json!({"name": "eng-ci", "scopes": ["build"], "team": "eng"})),
        )
        .await;
    assert_eq!(st, 201, "{k}");
    let team_key = k["secret"].as_str().unwrap().to_owned();
    assert_eq!(
        s.call(
            Method::POST,
            "/v1/agents",
            &team_key,
            None,
            Some(agent("helper"))
        )
        .await
        .0,
        201
    );
    // A builder can't hand out more than they have.
    assert_eq!(
        s.call(
            Method::POST,
            "/v1/keys",
            &builder,
            None,
            Some(json!({"name": "x", "scopes": ["admin"]}))
        )
        .await
        .0,
        403
    );

    // Signing out ends the token.
    assert_eq!(
        s.call(Method::GET, "/v1/me", &viewer, None, None).await.0,
        200
    );
    s.control
        .set_user_disabled(
            &s.control
                .find_user_by_email("vic@acme.test")
                .await
                .unwrap()
                .unwrap()
                .id,
            true,
        )
        .await
        .unwrap();
    assert_eq!(
        s.call(Method::GET, "/v1/me", &viewer, None, None).await.0,
        401
    );
}

#[tokio::test]
async fn a_cookie_sign_in_refuses_cross_site_writes() {
    let s = Server::start().await;
    let (token, _) = s.user("ada@example.com", DEFAULT_ORG, Role::Admin).await;
    let client = reqwest::Client::new();
    let r = client
        .get(format!("{}/v1/me", s.url))
        .header("cookie", format!("gai_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let me: Value = r.json().await.unwrap();
    assert_eq!(me["user"]["email"], "ada@example.com");
    let r = client
        .post(format!("{}/v1/agents", s.url))
        .header("cookie", format!("gai_session={token}"))
        .header("origin", "https://evil.example")
        .json(&json!({"name": "x", "model": "mock/echo"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let host = s.url.trim_start_matches("http://").to_owned();
    let r = client
        .post(format!("{}/v1/agents", s.url))
        .header("cookie", format!("gai_session={token}"))
        .header("origin", format!("http://{host}"))
        .json(&json!({"name": "x", "model": "mock/echo"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let r = client
        .post(format!("{}/v1/auth/logout", s.url))
        .header("cookie", format!("gai_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        r.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let r = client
        .get(format!("{}/v1/me", s.url))
        .header("cookie", format!("gai_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}
