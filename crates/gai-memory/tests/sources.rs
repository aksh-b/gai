//! Sources synced into knowledge: a folder for real, and the web, GitHub,
//! Confluence and Notion APIs mocked. Each sync after the first fetches
//! only what changed, and items that disappear are removed.

use std::sync::Arc;

use base64::Engine;
use gai_llm::{LlmConfig, ModelRegistry};
use gai_memory::sources::{Sources, SyncStats};
use gai_memory::{Knowledge, KnowledgeSettings, SearchMode};
use gai_store::{SharedStore, SqliteStore};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn sources() -> (Sources, Arc<Knowledge>) {
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let models = Arc::new(
        ModelRegistry::builder()
            .config(LlmConfig::default())
            .build()
            .unwrap(),
    );
    let k = Arc::new(Knowledge::new(store, models, KnowledgeSettings::default()));
    (Sources::new(k.clone()), k)
}

async fn save(s: &Sources, spec: Value) -> String {
    let spec = Sources::parse(spec).unwrap();
    s.save(&spec).await.unwrap();
    spec.name
}

fn counts(st: &SyncStats) -> (u64, u64, u64, u64, u64) {
    (st.added, st.updated, st.removed, st.unchanged, st.failed)
}

async fn sources_in(k: &Knowledge, collection: &str) -> Vec<String> {
    let mut v: Vec<String> = k
        .store()
        .list_documents(collection, 100, 0)
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.source)
        .collect();
    v.sort();
    v
}

#[tokio::test]
async fn a_folder_syncs_incrementally() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("leave.md"),
        "# Leave\n\nTwenty five days a year.",
    )
    .unwrap();
    std::fs::write(dir.path().join("pay.txt"), "Paid monthly.").unwrap();
    std::fs::write(dir.path().join("photo.bin"), [0u8, 1, 2]).unwrap();
    let (s, k) = sources().await;
    let name = save(
        &s,
        json!({"name": "hr", "collection": "handbook", "kind": "folder", "path": dir.path().to_str().unwrap()}),
    )
    .await;
    assert_eq!(counts(&s.sync(&name).await.unwrap()), (2, 0, 0, 0, 0));
    assert_eq!(counts(&s.sync(&name).await.unwrap()), (0, 0, 0, 2, 0));

    // Change one, delete the other.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(
        dir.path().join("leave.md"),
        "# Leave\n\nThirty days a year, from 2026.",
    )
    .unwrap();
    std::fs::remove_file(dir.path().join("pay.txt")).unwrap();
    assert_eq!(counts(&s.sync(&name).await.unwrap()), (0, 1, 1, 0, 0));
    let hits = k
        .search(&["handbook".into()], "thirty days", 3, SearchMode::Hybrid)
        .await
        .unwrap();
    assert!(hits[0].text.contains("Thirty"), "{hits:?}");
    let rec = s.get(&name).await.unwrap();
    assert_eq!(rec.status, "ok");
    assert_eq!(rec.stats["updated"], 1);
    assert!(s.due().await.unwrap().is_empty(), "no schedule");
}

#[tokio::test]
async fn web_pages_and_sitemaps() {
    let server = MockServer::start().await;
    let base = server.uri();
    let page = |title: &str, body: &str| {
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/html")
            .set_body_string(format!(
                "<html><head><title>{title}</title></head><body>{body}</body></html>"
            ))
    };
    Mock::given(path("/"))
        .respond_with(page("Home", r#"<p>Welcome.</p><a href="/docs">Docs</a> <a href="https://elsewhere.example/x">out</a>"#))
        .mount(&server)
        .await;
    Mock::given(path("/docs"))
        .respond_with(page(
            "Docs",
            r#"<p>Install with cargo.</p><a href="/deep">deeper</a>"#,
        ))
        .mount(&server)
        .await;
    Mock::given(path("/deep"))
        .respond_with(page("Deep", "<p>Too far.</p>"))
        .mount(&server)
        .await;
    Mock::given(path("/news"))
        .respond_with(page("News", "<p>Version 2 is out.</p>"))
        .mount(&server)
        .await;
    Mock::given(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "<urlset><url><loc>{base}/news</loc></url></urlset>"
        )))
        .mount(&server)
        .await;
    let (s, k) = sources().await;
    let name = save(
        &s,
        json!({"name": "site", "collection": "site", "kind": "web", "urls": [format!("{base}/")], "sitemap": format!("{base}/sitemap.xml"), "depth": 1, "allow_private": true}),
    )
    .await;
    let st = s.sync(&name).await.unwrap();
    let got = sources_in(&k, "site").await;
    assert_eq!(
        st.added, 3,
        "home, docs (depth 1), news (sitemap): {st:?} {got:?}"
    );
    let got = sources_in(&k, "site").await;
    assert!(
        got.iter().any(|u| u.ends_with("/docs")) && !got.iter().any(|u| u.ends_with("/deep")),
        "{got:?}"
    );
    assert_eq!(counts(&s.sync(&name).await.unwrap()), (0, 0, 0, 3, 0));

    // Private addresses need allow_private.
    let (s2, _) = sources().await;
    let n2 = save(
        &s2,
        json!({"name": "p", "collection": "p", "kind": "web", "urls": [format!("{base}/")]}),
    )
    .await;
    let st = s2.sync(&n2).await.unwrap();
    assert_eq!(st.added, 0, "{st:?}");
}

#[tokio::test]
async fn github_repositories() {
    let server = MockServer::start().await;
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    Mock::given(path("/repos/acme/docs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"default_branch": "main"})))
        .mount(&server)
        .await;
    let tree = |readme_sha: &str| {
        json!({"truncated": false, "tree": [
            {"path": "docs/guide.md", "type": "blob", "sha": "g1"},
            {"path": "docs/logo.png", "type": "blob", "sha": "l1"},
            {"path": "README.md", "type": "blob", "sha": readme_sha},
            {"path": "docs", "type": "tree", "sha": "t1"}
        ]})
    };
    Mock::given(path("/repos/acme/docs/git/trees/main"))
        .and(query_param("recursive", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tree("r1")))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(path("/repos/acme/docs/git/trees/main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tree("r2")))
        .mount(&server)
        .await;
    for (sha, text) in [
        ("g1", "# Guide\nRun the installer."),
        ("r1", "# Docs\nOld readme."),
        ("r2", "# Docs\nNew readme."),
    ] {
        Mock::given(path(format!("/repos/acme/docs/git/blobs/{sha}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"content": b64(text), "encoding": "base64"})),
            )
            .mount(&server)
            .await;
    }
    let (s, k) = sources().await;
    let name = save(
        &s,
        json!({"name": "repo", "collection": "repo", "kind": "github", "repo": "acme/docs", "api_url": server.uri()}),
    )
    .await;
    assert_eq!(
        counts(&s.sync(&name).await.unwrap()),
        (2, 0, 0, 0, 0),
        "the png is skipped"
    );
    assert_eq!(counts(&s.sync(&name).await.unwrap()), (0, 0, 0, 2, 0));
    assert_eq!(
        counts(&s.sync(&name).await.unwrap()),
        (0, 1, 0, 1, 0),
        "README changed"
    );
    assert_eq!(
        sources_in(&k, "repo").await,
        [
            "https://github.com/acme/docs/blob/main/README.md",
            "https://github.com/acme/docs/blob/main/docs/guide.md"
        ]
    );

    // Only a folder.
    let n2 = save(
        &s,
        json!({"name": "guides", "collection": "guides", "kind": "github", "repo": "acme/docs", "branch": "main", "path": "docs", "api_url": server.uri()}),
    )
    .await;
    assert_eq!(s.sync(&n2).await.unwrap().added, 1);
}

#[tokio::test]
async fn confluence_spaces() {
    let server = MockServer::start().await;
    let base = format!("{}/wiki", server.uri());
    Mock::given(path("/wiki/api/v2/spaces"))
        .and(query_param("keys", "HR"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"results": [{"id": "77", "key": "HR"}]})),
        )
        .mount(&server)
        .await;
    Mock::given(path("/wiki/api/v2/spaces/77/pages"))
        .and(query_param("cursor", "two"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
            {"id": "2", "title": "Expenses", "version": {"number": 1}, "body": {"storage": {"value": "<p>Receipts within 30 days.</p>"}}, "_links": {"webui": "/spaces/HR/pages/2"}}
        ], "_links": {}})))
        .mount(&server)
        .await;
    Mock::given(path("/wiki/api/v2/spaces/77/pages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
            {"id": "1", "title": "Leave", "version": {"number": 4}, "body": {"storage": {"value": "<h1>Leave</h1><p>25 days.</p>"}}, "_links": {"webui": "/spaces/HR/pages/1"}}
        ], "_links": {"next": "/wiki/api/v2/spaces/77/pages?cursor=two"}})))
        .mount(&server)
        .await;
    unsafe {
        std::env::set_var("GAI_TEST_CONF_EMAIL", "ada@example.com");
        std::env::set_var("GAI_TEST_CONF_TOKEN", "t0k");
    }
    let (s, k) = sources().await;
    let name = save(
        &s,
        json!({"name": "hr", "collection": "hr", "kind": "confluence", "base_url": base, "space": "HR", "email_env": "GAI_TEST_CONF_EMAIL", "token_env": "GAI_TEST_CONF_TOKEN"}),
    )
    .await;
    assert_eq!(
        counts(&s.sync(&name).await.unwrap()),
        (2, 0, 0, 0, 0),
        "both pages, over two requests"
    );
    let hits = k
        .search(&["hr".into()], "receipts", 2, SearchMode::Hybrid)
        .await
        .unwrap();
    assert!(hits[0].source.ends_with("/spaces/HR/pages/2"), "{hits:?}");
    let req = &server.received_requests().await.unwrap()[0];
    assert!(
        req.headers["authorization"]
            .to_str()
            .unwrap()
            .starts_with("Basic ")
    );

    // A missing token is a clear error.
    let n2 = save(
        &s,
        json!({"name": "x", "collection": "x", "kind": "confluence", "base_url": base, "space": "HR", "token_env": "GAI_TEST_UNSET_TOKEN"}),
    )
    .await;
    let err = s.sync(&n2).await.unwrap_err().to_string();
    assert!(err.contains("GAI_TEST_UNSET_TOKEN"), "{err}");
    assert_eq!(s.get(&n2).await.unwrap().status, "failed");
}

#[tokio::test]
async fn notion_databases() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/databases/db1/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"has_more": false, "results": [
            {"id": "aaaa-1111", "url": "https://www.notion.so/Onboarding-aaaa1111", "last_edited_time": "2026-10-01T00:00:00Z",
             "properties": {"Name": {"type": "title", "title": [{"plain_text": "Onboarding"}]}}},
            {"id": "bbbb-2222", "url": "https://www.notion.so/Old-bbbb2222", "archived": true, "last_edited_time": "2026-01-01T00:00:00Z", "properties": {}}
        ]})))
        .mount(&server)
        .await;
    Mock::given(path("/v1/blocks/aaaa1111/children"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"has_more": false, "results": [
            {"id": "h", "type": "heading_2", "heading_2": {"rich_text": [{"plain_text": "First week"}]}},
            {"id": "p", "type": "paragraph", "has_children": true, "paragraph": {"rich_text": [{"plain_text": "Meet your buddy."}]}},
            {"id": "t", "type": "to_do", "to_do": {"checked": true, "rich_text": [{"plain_text": "Laptop"}]}}
        ]})))
        .mount(&server)
        .await;
    Mock::given(path("/v1/blocks/p/children"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"has_more": false, "results": [
            {"id": "q", "type": "quote", "quote": {"rich_text": [{"plain_text": "Ask questions."}]}}
        ]})))
        .mount(&server)
        .await;
    unsafe { std::env::set_var("GAI_TEST_NOTION", "secret_x") };
    let (s, k) = sources().await;
    let name = save(
        &s,
        json!({"name": "wiki", "collection": "wiki", "kind": "notion", "token_env": "GAI_TEST_NOTION", "database_id": "db1", "api_url": server.uri()}),
    )
    .await;
    assert_eq!(
        counts(&s.sync(&name).await.unwrap()),
        (1, 0, 0, 0, 0),
        "archived pages are skipped"
    );
    let hits = k
        .search(
            &["wiki".into()],
            "buddy questions laptop",
            3,
            SearchMode::Text,
        )
        .await
        .unwrap();
    let text: String = hits.iter().map(|h| h.text.clone()).collect();
    for want in [
        "First week",
        "Meet your buddy",
        "[x] Laptop",
        "Ask questions",
    ] {
        assert!(text.contains(want), "{want} in {text}");
    }
    assert_eq!(counts(&s.sync(&name).await.unwrap()), (0, 0, 0, 1, 0));
}

#[test]
fn bad_specs() {
    assert!(Sources::parse(json!({"name": "x", "collection": "y", "kind": "ftp"})).is_err());
    assert!(
        Sources::parse(
            json!({"name": "bad name", "collection": "y", "kind": "folder", "path": "/"})
        )
        .is_err()
    );
    assert!(
        Sources::parse(json!({"name": "x", "collection": "y", "kind": "github", "repo": "nope", }))
            .is_ok(),
        "checked on sync"
    );
}
