//! Runs code in real containers. Needs a Docker/Podman socket, so it is
//! ignored by default; `scripts/sandbox-test.sh` passes the podman socket
//! through and runs it.

#![cfg(feature = "container")]

use std::time::Duration;

use gai_sandbox::container::ContainerSandbox;
use gai_sandbox::{ExecRequest, Language, Sandbox, SandboxSettings};
use tokio_util::sync::CancellationToken;

fn req(dir: &std::path::Path, language: Language, code: &str, secs: u64) -> ExecRequest {
    ExecRequest {
        language,
        code: code.into(),
        workspace: dir.to_path_buf(),
        timeout: Duration::from_secs(secs),
        run_id: "run_sandbox_test".into(),
    }
}

#[tokio::test]
#[ignore = "needs a container engine (scripts/sandbox-test.sh)"]
async fn runs_code_in_an_isolated_container() {
    let sb = ContainerSandbox::new(SandboxSettings::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::write(dir.path().join("data/nums.txt"), "1\n2\n3\n").unwrap();
    std::fs::write(dir.path().join("scratch.txt"), "delete me").unwrap();

    // Reads the workspace, writes into a new folder, deletes a file, and
    // finds no network.
    let code = r#"
import os, socket
nums = [int(x) for x in open("data/nums.txt")]
print("sum", sum(nums))
os.makedirs("out", exist_ok=True)
open("out/result.txt", "w").write(str(sum(nums) * 2))
os.remove("scratch.txt")
try:
    socket.create_connection(("1.1.1.1", 53), timeout=3)
    print("network: yes")
except OSError:
    print("network: no")
print("uid", os.getuid())
import sys; print("oops", file=sys.stderr)
"#;
    let r = sb
        .exec(
            req(dir.path(), Language::Python, code, 60),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(r.exit_code, Some(0), "{r:?}");
    assert!(r.stdout.contains("sum 6"), "{}", r.stdout);
    assert!(r.stdout.contains("network: no"), "{}", r.stdout);
    assert!(r.stdout.contains("uid 1000"), "{}", r.stdout);
    assert_eq!(r.stderr.trim(), "oops");
    assert_eq!(r.changed, ["out/result.txt"]);
    assert_eq!(r.deleted, ["scratch.txt"]);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("out/result.txt")).unwrap(),
        "12"
    );
    assert!(!dir.path().join("scratch.txt").exists());

    // Errors keep their exit code; bash runs in the same image.
    let r = sb
        .exec(
            req(dir.path(), Language::Bash, "cat out/result.txt; exit 3", 30),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(r.exit_code, Some(3));
    assert_eq!(r.stdout, "12");
    assert!(r.changed.is_empty() && r.deleted.is_empty(), "{r:?}");

    // A runaway program is stopped at the time limit.
    let r = sb
        .exec(
            req(
                dir.path(),
                Language::Python,
                "import time\nprint('start', flush=True)\ntime.sleep(60)",
                3,
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(r.timed_out);
    assert_eq!(r.exit_code, None);
    assert!(r.stdout.contains("start"), "{r:?}");
}

#[tokio::test]
#[ignore = "needs a container engine (scripts/sandbox-test.sh)"]
async fn runs_node() {
    let sb = ContainerSandbox::new(SandboxSettings::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let r = sb
        .exec(
            req(
                dir.path(),
                Language::Node,
                "require('fs').writeFileSync('hi.json', JSON.stringify({n: 6*7})); console.log('ok')",
                60,
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(r.exit_code, Some(0), "{r:?}");
    assert_eq!(r.stdout.trim(), "ok");
    assert_eq!(r.changed, ["hi.json"]);
}

#[tokio::test]
async fn without_an_engine_calls_say_what_is_missing() {
    let settings = SandboxSettings {
        engine: Some("unix:///nonexistent/docker.sock".into()),
        ..Default::default()
    };
    let sb = ContainerSandbox::new(settings).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let e = sb
        .exec(
            req(dir.path(), Language::Python, "print(1)", 10),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("container engine"), "{e}");
}
