//! Talking to agents over `/v1/voice`: the cascade (transcribe, run, speak
//! by sentence) with mock models, a realtime relay to a fake provider whose
//! tool call runs here, and a recording uploaded and processed over HTTP.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use futures::{SinkExt, StreamExt};
use gai::llm::{LlmConfig, ProviderConfig};
use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

async fn serve(config: Config) -> String {
    let gai = Arc::new(Gai::from_config(config, None).unwrap());
    let app = router(gai, AuthPolicy::new(AuthMode::Off, true, 0), None)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("{addr}")
}

fn config(dir: &std::path::Path) -> Config {
    let mut config = Config {
        data_dir: Some(dir.join("data")),
        default_model: Some("mock/echo".into()),
        ..Default::default()
    };
    config.media.transcription = Some("mock/ears".into());
    config.media.speech = Some("mock/voice".into());
    config
}

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The next JSON message, skipping none.
async fn next(ws: &mut Client) -> Value {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(20), ws.next())
            .await
            .expect("a message in time")
            .expect("open")
            .unwrap();
        if let Message::Text(t) = m {
            return serde_json::from_str(t.as_str()).unwrap();
        }
    }
}

/// Messages up to and including the first of type `until`.
async fn until(ws: &mut Client, until: &str) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        let v = next(ws).await;
        let done = v["type"] == until;
        assert_ne!(v["type"], "error", "{v}");
        seen.push(v);
        if done {
            return seen;
        }
    }
}

async fn send(ws: &mut Client, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

#[tokio::test]
async fn cascade_voice_turns_share_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let addr = serve(config(dir.path())).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/voice"))
        .await
        .unwrap();
    send(&mut ws, json!({"type": "start"})).await;
    let ready = next(&mut ws).await;
    assert_eq!(
        (ready["type"].as_str(), ready["mode"].as_str()),
        (Some("ready"), Some("cascade"))
    );
    assert_eq!(ready["speech"], "mock/voice");

    let said = "Tell me something interesting, please. And keep it short.";
    ws.send(Message::Binary(
        gai::llm::dialect::mock::mock_wav(said, 2.0).into(),
    ))
    .await
    .unwrap();
    send(
        &mut ws,
        json!({"type": "commit", "media_type": "audio/wav"}),
    )
    .await;
    let turn = until(&mut ws, "reply_done").await;
    assert_eq!(turn[0]["type"], "transcript");
    assert_eq!(turn[0]["text"], said);
    let started = turn.iter().find(|v| v["type"] == "reply_started").unwrap();
    let text: String = turn
        .iter()
        .filter(|v| v["type"] == "delta")
        .map(|v| v["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(text, said, "mock/echo says it back");
    let audio: Vec<&Value> = turn.iter().filter(|v| v["type"] == "audio").collect();
    assert_eq!(audio.len(), 2, "one clip per sentence: {turn:?}");
    assert_eq!(audio[0]["text"], "Tell me something interesting, please.");
    assert_eq!(audio[1]["seq"], 1);
    let wav = base64::engine::general_purpose::STANDARD
        .decode(audio[1]["data"].as_str().unwrap())
        .unwrap();
    assert!(wav.starts_with(b"RIFF"));
    let done = turn.last().unwrap();
    assert_eq!(done["status"], "completed");
    let session = done["session_id"].as_str().unwrap().to_owned();
    assert_eq!(started["session_id"], session);

    send(
        &mut ws,
        json!({"type": "text", "text": "Second turn here, thank you."}),
    )
    .await;
    let turn = until(&mut ws, "reply_done").await;
    assert_eq!(
        turn.last().unwrap()["session_id"],
        session,
        "same conversation"
    );

    // Nothing to interrupt is fine; committing nothing is an error.
    send(&mut ws, json!({"type": "interrupt"})).await;
    send(&mut ws, json!({"type": "commit"})).await;
    let e = next(&mut ws).await;
    assert_eq!(e["type"], "error");
    ws.close(None).await.unwrap();
}

#[tokio::test]
// tungstenite's handshake callback returns its large error type.
#[allow(clippy::result_large_err)]
async fn realtime_sessions_relay_audio_and_run_tools_here() {
    // A fake provider realtime endpoint.
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up_addr = upstream.local_addr().unwrap();
    let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    tokio::spawn(async move {
        let (tcp, _) = upstream.accept().await.unwrap();
        let path = Arc::new(std::sync::Mutex::new(String::new()));
        let p2 = path.clone();
        let mut ws = tokio_tungstenite::accept_hdr_async(
            tcp,
            move |req: &tokio_tungstenite::tungstenite::handshake::server::Request, resp| {
                *p2.lock().unwrap() = req.uri().to_string();
                Ok(resp)
            },
        )
        .await
        .unwrap();
        let _ = seen_tx.send(json!({"path": *path.lock().unwrap()}));
        let mut read = async || loop {
            if let Some(Ok(Message::Text(t))) = ws.next().await {
                return serde_json::from_str::<Value>(t.as_str()).unwrap();
            }
        };
        let update = read().await;
        let _ = seen_tx.send(update);
        let append = read().await;
        let _ = seen_tx.send(append);
        drop(read);
        for ev in [
            json!({"type": "input_audio_buffer.speech_started"}),
            json!({"type": "conversation.item.input_audio_transcription.completed", "transcript": "what is six times seven"}),
            json!({"type": "response.created"}),
            json!({"type": "response.output_audio.delta", "delta": "AAAA"}),
            json!({"type": "response.output_audio_transcript.delta", "delta": "Let me work it out."}),
            json!({"type": "response.function_call_arguments.done", "call_id": "c1", "name": "calculator", "arguments": "{\"expression\": \"6*7\"}"}),
            json!({"type": "response.done", "response": {"status": "completed", "usage": {"input_tokens": 10, "output_tokens": 5}}}),
        ] {
            ws.send(Message::Text(ev.to_string().into())).await.unwrap();
        }
        let mut read = async || loop {
            if let Some(Ok(Message::Text(t))) = ws.next().await {
                return serde_json::from_str::<Value>(t.as_str()).unwrap();
            }
        };
        let output = read().await;
        let _ = seen_tx.send(output);
        let create = read().await;
        let _ = seen_tx.send(create);
        drop(read);
        for ev in [
            json!({"type": "response.created"}),
            json!({"type": "response.output_audio_transcript.delta", "delta": "It is 42."}),
            json!({"type": "response.done", "response": {"status": "completed"}}),
        ] {
            ws.send(Message::Text(ev.to_string().into())).await.unwrap();
        }
        // Stay open until the client goes.
        while ws.next().await.is_some() {}
    });

    let dir = tempfile::tempdir().unwrap();
    let mut config = config(dir.path());
    let mut llm = LlmConfig::default();
    llm.providers.insert(
        "rt".into(),
        ProviderConfig::new("openai", &format!("http://{up_addr}/v1")),
    );
    config.llm.providers.extend(llm.providers);
    let addr = serve(config).await;
    let c = reqwest::Client::new();
    let r = c
        .post(format!("http://{addr}/v1/agents"))
        .json(&json!({
            "name": "talker",
            "model": "mock/echo",
            "system": "Be brief.",
            "tools": ["calculator", "shell"],
            "media": {"realtime": "rt/gpt-realtime"},
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.text().await.unwrap());

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/voice"))
        .await
        .unwrap();
    send(&mut ws, json!({"type": "start", "agent": "talker"})).await;
    let ready = next(&mut ws).await;
    assert_eq!(ready["mode"], "realtime", "{ready}");
    assert_eq!(ready["input"]["rate"], 24000);
    let path = seen_rx.recv().await.unwrap();
    assert_eq!(path["path"], "/v1/realtime?model=gpt-realtime");
    let update = seen_rx.recv().await.unwrap();
    assert_eq!(update["type"], "session.update");
    assert_eq!(update["session"]["instructions"], "Be brief.");
    let tools: Vec<&str> = update["session"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        tools,
        ["calculator"],
        "shell needs approval, so it is left out"
    );

    ws.send(Message::Binary(vec![1u8, 0, 2, 0].into()))
        .await
        .unwrap();
    let append = seen_rx.recv().await.unwrap();
    assert_eq!(append["type"], "input_audio_buffer.append");
    assert_eq!(append["audio"], "AQACAA==");

    let first = until(&mut ws, "tool").await;
    let kinds: Vec<&str> = first.iter().map(|v| v["type"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        [
            "speech_started",
            "transcript",
            "reply_started",
            "audio",
            "delta",
            "tool"
        ]
    );
    assert_eq!(first[1]["text"], "what is six times seven");
    assert_eq!(first[3]["media_type"], "audio/pcm;rate=24000");

    let output = seen_rx.recv().await.unwrap();
    assert_eq!(output["item"]["type"], "function_call_output");
    assert_eq!(output["item"]["call_id"], "c1");
    assert_eq!(output["item"]["output"], "42");
    let create = seen_rx.recv().await.unwrap();
    assert_eq!(create["type"], "response.create", "the answer goes on");

    let rest = until(&mut ws, "reply_done").await;
    assert_eq!(rest.last().unwrap()["text"], "It is 42.");
    ws.close(None).await.unwrap();
}

#[tokio::test]
async fn recordings_upload_and_process_in_the_background() {
    let dir = tempfile::tempdir().unwrap();
    let addr = serve(config(dir.path())).await;
    let c = reqwest::Client::new();
    let blob: Value = c
        .post(format!("http://{addr}/v1/blobs?name=call.wav"))
        .header("content-type", "audio/wav")
        .body(gai::llm::dialect::mock::mock_wav(
            "Ana: Shall we start?\nBen: Yes, the numbers look good.\nAna: Great.",
            4.0,
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let r = c
        .post(format!("http://{addr}/v1/recordings"))
        .json(&json!({"blob_id": blob["id"], "kind": "audio"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 202);
    let rec: Value = r.json().await.unwrap();
    assert_eq!(rec["title"], "call.wav");
    let id = rec["id"].as_str().unwrap();
    let done = loop {
        let v: Value = c
            .get(format!("http://{addr}/v1/recordings/{id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if v["status"] != "processing" {
            break v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(done["status"], "done", "{done}");
    // mock/echo can't analyse, so only the transcript's own facts are there.
    let speakers = done["result"]["insights"]["speakers"].as_array().unwrap();
    assert_eq!(speakers[0]["name"], "Ben", "most talk time first");
    let ana = speakers.iter().find(|s| s["name"] == "Ana").unwrap();
    assert_eq!(ana["turns"], 2);
    assert_eq!(done["result"]["insights"]["language"], "en");
    let list: Value = c
        .get(format!("http://{addr}/v1/recordings"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list[0]["result"]["duration_secs"].as_f64().unwrap() > 3.0);
    assert!(
        list[0]["result"].get("transcript").is_none(),
        "lists stay small"
    );
    let templates: Value = c
        .get(format!("http://{addr}/v1/recordings/templates"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        templates
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "standup")
    );
    let bad = c
        .post(format!("http://{addr}/v1/recordings"))
        .json(&json!({"blob_id": "blob_nope"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
}

#[tokio::test]
async fn interrupting_stops_the_reply_and_its_run() {
    // A page that takes a long time, for a reply still in flight.
    let slow = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let slow_addr = slow.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = slow.accept().await.unwrap();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(30)).await;
                drop(s);
            });
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(dir.path());
    config.tools.allow_private_network = true;
    let addr = serve(config).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/voice"))
        .await
        .unwrap();
    send(&mut ws, json!({"type": "start"})).await;
    assert_eq!(next(&mut ws).await["type"], "ready");
    send(
        &mut ws,
        json!({"type": "text", "text": format!("call:http_fetch {{\"url\": \"http://{slow_addr}/\"}}")}),
    )
    .await;
    let seen = until(&mut ws, "tool").await;
    let run_id = seen.iter().find(|v| v["type"] == "reply_started").unwrap()["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // The user talks over the agent.
    send(&mut ws, json!({"type": "interrupt"})).await;
    let stopped = until(&mut ws, "interrupted").await;
    assert_eq!(stopped.last().unwrap()["run_id"], run_id);
    let c = reqwest::Client::new();
    let status = loop {
        let v: Value = c
            .get(format!("http://{addr}/v1/runs/{run_id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if v["active"] == false {
            break v["status"].clone();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_ne!(status, "completed", "the run was stopped: {status}");
    // And the next turn goes ahead, in the same conversation, with the
    // cut-off tool call closed (mock/echo reports it back).
    send(
        &mut ws,
        json!({"type": "text", "text": "Are you still there?"}),
    )
    .await;
    let turn = until(&mut ws, "reply_done").await;
    let done = turn.last().unwrap();
    assert_eq!(done["status"], "completed");
    assert!(
        done["text"].as_str().unwrap().contains("interrupted"),
        "{done}"
    );
}
