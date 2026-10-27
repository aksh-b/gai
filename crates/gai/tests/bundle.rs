//! `.gaibundle` round trip: export an agent with everything it uses from
//! one gai, import it into another, and get the same specs back, with no
//! secrets in the archive.

use std::io::Read;

use gai::agent::{AgentSpec, PersonaSpec};
use gai::bundle::{ExportOptions, ImportOptions};
use gai::store::NewBlock;
use serde_json::json;

fn gai_in(dir: &std::path::Path) -> gai::Gai {
    gai::Gai::builder()
        .with_config(gai::Config::default())
        .data_dir(dir)
        .default_model("mock/echo")
        .build()
        .unwrap()
}

fn files(bundle: &[u8]) -> Vec<(String, String)> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bundle)).unwrap();
    (0..z.len())
        .map(|i| {
            let mut f = z.by_index(i).unwrap();
            let mut s = String::new();
            f.read_to_string(&mut s).unwrap();
            (f.name().to_owned(), s)
        })
        .collect()
}

#[tokio::test]
async fn bundles_round_trip_without_secrets() {
    let a = tempfile::tempdir().unwrap();
    let src = gai_in(a.path());
    let rt = src.runtime().await.unwrap();
    let store = rt.store();

    let persona: PersonaSpec =
        serde_json::from_value(json!({"name": "tutor", "display_name": "Tutor", "tone": "warm"}))
            .unwrap();
    store
        .put_persona("tutor", &serde_json::to_value(&persona).unwrap(), None)
        .await
        .unwrap();
    rt.skills()
        .save(
            "---\nname: haiku\ndescription: Write haiku.\n---\nThree lines: 5, 7, 5.",
            [("examples.txt".to_owned(), "old pond".to_owned())].into(),
        )
        .await
        .unwrap();
    store
        .put_mcp_server(
            "tracker",
            &json!({"url": "https://mcp.example.com/mcp", "headers": {"Authorization": "Bearer sk-secret-123"}}),
        )
        .await
        .unwrap();
    rt.knowledge()
        .create_collection(gai::memory::NewCollection {
            name: "handbook".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    rt.knowledge()
        .ingest(
            "handbook",
            gai::memory::DocumentInput {
                source: "leave.md".into(),
                title: Some("Leave".into()),
                text: "Everyone gets 25 days of leave a year.".into(),
                metadata: json!({}),
            },
        )
        .await
        .unwrap();

    rt.save_prompt(
        &gai::agent::PromptSpec::parse("name: tone\ntext: Be brief.\n", "yaml").unwrap(),
        None,
    )
    .await
    .unwrap();
    store
        .set_item_label(gai::agent::PROMPT_KIND, "tone", "prod", Some(1))
        .await
        .unwrap();

    let helper = AgentSpec::new("helper").system("Help.");
    rt.save_agent(&helper, None).await.unwrap();
    let mut main: AgentSpec = serde_json::from_value(json!({
        "name": "main",
        "model": "mock/echo",
        "system": "Be useful.",
        "persona": "tutor",
        "prompt": "tone@prod",
        "skills": ["haiku"],
        "knowledge": ["handbook"],
        "tools": ["calculator", "mcp__tracker__*"],
        "delegates": ["helper"]
    }))
    .unwrap();
    main.description = Some("The main one".into());
    rt.save_agent(&main, None).await.unwrap();
    store
        .create_block(
            NewBlock {
                scope: "agent:main".into(),
                label: "notes".into(),
                value: "likes tea".into(),
                limit: 500,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    let bundle = gai::bundle::export(
        &src,
        &ExportOptions {
            name: Some("Main kit".into()),
            agents: vec!["main".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let listing = files(&bundle);
    let names: Vec<&str> = listing.iter().map(|(n, _)| n.as_str()).collect();
    for want in [
        "bundle.toml",
        "agents/main.yaml",
        "agents/helper.yaml",
        "personas/tutor.yaml",
        "prompts/tone.yaml",
        "prompts/tone.labels",
        "skills/haiku/SKILL.md",
        "skills/haiku/examples.txt",
        "mcp/tracker.json",
        "memory/blocks.json",
        "knowledge/handbook/collection.json",
        "knowledge/handbook/00001.md",
    ] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    assert!(
        listing
            .iter()
            .all(|(_, body)| !body.contains("sk-secret-123")),
        "a secret leaked into the bundle"
    );
    let manifest = &listing.iter().find(|(n, _)| n == "bundle.toml").unwrap().1;
    assert!(
        manifest.contains("GAI_MCP_TRACKER_AUTHORIZATION"),
        "{manifest}"
    );

    // Into a fresh gai.
    let b = tempfile::tempdir().unwrap();
    let dst = gai_in(b.path());
    let peek = gai::bundle::inspect(&dst, &bundle).await.unwrap();
    assert_eq!(peek.manifest.name, "Main kit");
    assert!(
        peek.missing_env
            .iter()
            .any(|v| v.name == "GAI_MCP_TRACKER_AUTHORIZATION")
    );
    let report = gai::bundle::import(&dst, &bundle, &ImportOptions::default())
        .await
        .unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let drt = dst.runtime().await.unwrap();
    let (got, _) = dst.find_agent("main").await.unwrap();
    assert_eq!(got, main);
    assert!(dst.find_agent("helper").await.is_ok());
    assert_eq!(
        drt.store()
            .get_persona("tutor", None)
            .await
            .unwrap()
            .unwrap()
            .spec["tone"],
        "warm"
    );
    let (tone, _) = drt.resolve_prompt("tone@prod").await.unwrap();
    assert_eq!(tone.text, "Be brief.");
    let skill = drt.skills().get("haiku").await.unwrap().unwrap();
    assert_eq!(skill.read_file("examples.txt").unwrap(), b"old pond");
    let mcp = drt.store().list_mcp_servers().await.unwrap();
    assert_eq!(
        mcp[0].1["headers"]["Authorization"],
        "${GAI_MCP_TRACKER_AUTHORIZATION}"
    );
    let block = drt
        .store()
        .find_block("agent:main", "notes")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(block.value, "likes tea");
    let hits = drt
        .knowledge()
        .search(
            &["handbook".into()],
            "how many days of leave",
            3,
            Default::default(),
        )
        .await
        .unwrap();
    assert!(hits.iter().any(|h| h.text.contains("25 days")), "{hits:?}");

    // Again with skip_existing: nothing new.
    let again = gai::bundle::import(
        &dst,
        &bundle,
        &ImportOptions {
            skip_existing: true,
        },
    )
    .await
    .unwrap();
    assert!(again.installed.is_empty(), "{:?}", again.installed);

    // Personal data can be masked on the way out.
    store
        .set_block_value(
            &store
                .find_block("agent:main", "notes")
                .await
                .unwrap()
                .unwrap()
                .id,
            "call ada on +44 20 7946 0958",
            None,
            "replace",
            None,
        )
        .await
        .unwrap();
    let redacted = gai::bundle::export(
        &src,
        &ExportOptions {
            agents: vec!["main".into()],
            redact_pii: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let blocks = &files(&redacted)
        .into_iter()
        .find(|(n, _)| n == "memory/blocks.json")
        .unwrap()
        .1;
    assert!(
        blocks.contains("[phone]") && !blocks.contains("7946"),
        "{blocks}"
    );

    // Not a bundle, or a hostile one.
    assert!(gai::bundle::inspect(&dst, b"not a zip").await.is_err());
}
