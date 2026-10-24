//! `gai::client::Client` against a real server: runs, streams, errors.

use std::sync::Arc;

use futures::StreamExt;
use gai::client::{Client, ClientError, RunOptions};
use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};

async fn server() -> (Client, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        data_dir: Some(dir.path().to_path_buf()),
        default_model: Some("mock/echo".into()),
        ..Default::default()
    };
    let gai = Arc::new(Gai::from_config(config, None).unwrap());
    gai.runtime()
        .await
        .unwrap()
        .save_agent(
            &gai::agent::AgentSpec::new("echoer").tools(["calculator"]),
            None,
        )
        .await
        .unwrap();
    let app = router(gai, AuthPolicy::new(AuthMode::Off, true, 0), None)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (Client::new(url), dir)
}

#[tokio::test]
async fn client_runs_streams_and_reports_errors() {
    let (c, _dir) = server().await;
    assert_eq!(c.health().await.unwrap()["status"], "ok");
    assert!(
        c.agents()
            .await
            .unwrap()
            .iter()
            .any(|a| a["summary"]["name"] == "echoer")
    );

    let out = c.run("echoer", "hello").await.unwrap();
    assert!(out.text.contains("hello"), "{}", out.text);

    // Same session, streamed, with a tool call.
    let mut events = c
        .stream(
            RunOptions::agent("echoer").session(&out.session_id),
            r#"call:calculator {"expression": "6*7"}"#,
        )
        .await
        .unwrap();
    let mut kinds = Vec::new();
    let mut run_id = String::new();
    while let Some(e) = events.next().await {
        let e = e.unwrap();
        run_id = e.run_id.clone();
        kinds.push(
            serde_json::to_value(&e.event).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert!(kinds.iter().any(|k| k.contains("tool")), "{kinds:?}");
    let run = c.get_run(&run_id).await.unwrap();
    assert_eq!(run["session_id"], out.session_id);

    let err = c.run("missing", "x").await.unwrap_err();
    assert!(
        matches!(err, ClientError::Api { status, .. } if status == 404),
        "{err}"
    );
}
