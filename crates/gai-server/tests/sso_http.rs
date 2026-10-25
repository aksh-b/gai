//! Signing in with OIDC against a mock identity provider, and through a
//! trusted proxy header.

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use gai::store::Role;
use gai::{AuthMode, Config, Gai, OidcSettings, SsoSettings, TrustedHeader};
use gai_server::{AuthPolicy, router};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn server(sso: SsoSettings) -> (String, Arc<Gai>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config {
        data_dir: Some(dir.path().to_path_buf()),
        default_model: Some("mock/echo".into()),
        ..Default::default()
    };
    config.server.sso = sso;
    let gai = Arc::new(Gai::from_config(config, None).unwrap());
    let app = router(
        gai.clone(),
        AuthPolicy::new(AuthMode::Required, true, 0),
        None,
    )
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, gai, dir)
}

fn jwt(claims: &Value) -> String {
    let enc = |v: &Value| URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).unwrap());
    format!("{}.{}.sig", enc(&json!({"alg": "RS256"})), enc(claims))
}

fn no_redirects() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// Start a sign-in and return the provider URL's query.
async fn start(url: &str) -> std::collections::HashMap<String, String> {
    let r = no_redirects()
        .get(format!("{url}/v1/auth/login?return_to=/runs"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303, "{}", r.text().await.unwrap());
    let to = url::Url::parse(r.headers()["location"].to_str().unwrap()).unwrap();
    to.query_pairs().into_owned().collect()
}

#[tokio::test]
async fn oidc_sign_in_creates_the_user_and_a_session() {
    let idp = MockServer::start().await;
    let issuer = idp.uri();
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
        })))
        .mount(&idp)
        .await;
    let (url, gai, _dir) = server(SsoSettings {
        oidc: Some(OidcSettings {
            issuer: issuer.clone(),
            client_id: "gai".into(),
            ..Default::default()
        }),
        admins: vec!["ada@example.com".into()],
        allowed_domains: vec!["example.com".into()],
        ..Default::default()
    })
    .await;

    let cfg: Value = reqwest::get(format!("{url}/v1/auth/config"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cfg["oidc"], true);

    let q = start(&url).await;
    assert_eq!(q["client_id"], "gai");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["redirect_uri"], format!("{url}/v1/auth/callback"));
    let claims = |nonce: &str, email: &str, aud: &str| {
        json!({
            "iss": issuer, "sub": "u-1", "aud": aud, "exp": chrono::Utc::now().timestamp() + 300,
            "nonce": nonce, "email": email, "email_verified": true, "name": "Ada",
        })
    };
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("code=good"))
        .and(body_string_contains("code_verifier="))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at", "token_type": "Bearer",
            "id_token": jwt(&claims(&q["nonce"], "ada@example.com", "gai")),
        })))
        .mount(&idp)
        .await;

    let r = no_redirects()
        .get(format!(
            "{url}/v1/auth/callback?code=good&state={}",
            q["state"]
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303, "{}", r.text().await.unwrap());
    assert_eq!(r.headers()["location"], "/runs");
    let cookie = r.headers()["set-cookie"].to_str().unwrap().to_owned();
    assert!(
        cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"),
        "{cookie}"
    );
    let session = cookie.split(';').next().unwrap().to_owned();

    // The state is single-use.
    let again = no_redirects()
        .get(format!(
            "{url}/v1/auth/callback?code=good&state={}",
            q["state"]
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 401);

    let me: Value = reqwest::Client::new()
        .get(format!("{url}/v1/me"))
        .header("cookie", &session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["user"]["email"], "ada@example.com");
    assert_eq!(me["principal"]["role"], "owner");
    let store = gai.runtime().await.unwrap().store().clone();
    let ada = store
        .find_user_by_email("ada@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store.user_memberships(&ada.id).await.unwrap()[0].role,
        Role::Owner
    );

    // A wrong nonce, audience or domain is refused.
    for (nonce, email, aud) in [
        ("other", "ada@example.com", "gai"),
        ("", "ada@example.com", "someone-else"),
        ("", "eve@elsewhere.test", "gai"),
    ] {
        let q = start(&url).await;
        let nonce = if nonce.is_empty() {
            q["nonce"].clone()
        } else {
            nonce.to_owned()
        };
        let code = format!("c{}", &q["state"][..8]);
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains(format!("code={code}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id_token": jwt(&claims(&nonce, email, aud)),
            })))
            .mount(&idp)
            .await;
        let r = no_redirects()
            .get(format!(
                "{url}/v1/auth/callback?code={code}&state={}",
                q["state"]
            ))
            .send()
            .await
            .unwrap();
        assert!(
            matches!(r.status().as_u16(), 401 | 403),
            "{email} {aud}: {}",
            r.status()
        );
    }

    // Not invited and no default role: signed in, but in no organization.
    let q = start(&url).await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("code=bob"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id_token": jwt(&json!({
                "iss": issuer, "sub": "u-2", "aud": "gai", "exp": chrono::Utc::now().timestamp() + 300,
                "nonce": q["nonce"], "email": "bob@example.com",
            })),
        })))
        .mount(&idp)
        .await;
    let r = no_redirects()
        .get(format!(
            "{url}/v1/auth/callback?code=bob&state={}",
            q["state"]
        ))
        .send()
        .await
        .unwrap();
    let bob = r.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let c = reqwest::Client::new();
    let me = c
        .get(format!("{url}/v1/me"))
        .header("cookie", &bob)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);
    let me: Value = me.json().await.unwrap();
    assert_eq!(me["memberships"], json!([]));
    let agents = c
        .get(format!("{url}/v1/agents"))
        .header("cookie", &bob)
        .send()
        .await
        .unwrap();
    assert_eq!(agents.status(), 403);
}

#[tokio::test]
async fn a_trusted_proxy_header_signs_people_in() {
    let (url, _gai, _dir) = server(SsoSettings {
        trusted_header: Some(TrustedHeader {
            email: "x-forwarded-email".into(),
            name: None,
        }),
        default_role: Some(Role::Member),
        ..Default::default()
    })
    .await;
    let c = reqwest::Client::new();
    let me: Value = c
        .get(format!("{url}/v1/me"))
        .header("x-forwarded-email", "cy@example.com")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["user"]["email"], "cy@example.com");
    assert_eq!(me["principal"]["role"], "member");
    let r = c
        .post(format!("{url}/v1/agents"))
        .header("x-forwarded-email", "cy@example.com")
        .json(&json!({"name": "x", "model": "mock/echo"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    // Without the header there is no one.
    assert_eq!(
        c.get(format!("{url}/v1/me")).send().await.unwrap().status(),
        401
    );
}
