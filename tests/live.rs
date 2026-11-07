//! Against the real `claude` (costs a few cents, needs a login). Run on purpose:
//!   SWB_LIVE=1 cargo test --test live -- --ignored
use std::time::Duration;

use swb_app::registry::Registry;
use swb_app::session::Envelope;
use tokio::sync::broadcast::Receiver;

async fn until(rx: &mut Receiver<Envelope>, what: impl Fn(&Envelope) -> bool) -> Envelope {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(90), rx.recv()).await.expect("timed out").expect("the stream ended");
        if what(&got) {
            return got;
        }
    }
}

#[tokio::test]
#[ignore = "uses the real claude; run with SWB_LIVE=1 and --ignored"]
async fn live_a_permission_is_asked_and_an_approved_command_runs() {
    if std::env::var_os("SWB_LIVE").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let registry = Registry::open(dir.path().join("data"), None).unwrap();
    let session = registry.create(project.to_str().unwrap(), Some("haiku".into())).unwrap();
    let (_, mut rx) = session.subscribe(0).await.unwrap();
    session.send("Create an empty file named live.txt in the current directory with the Bash tool (touch live.txt).".into(), vec![]).await.unwrap();

    let ask = until(&mut rx, |e| e.kind == "permission").await;
    assert_eq!(ask.data["tool_name"], "Bash");
    assert!(!project.join("live.txt").exists(), "nothing runs before the answer");
    session.permission(ask.data["request_id"].as_str().unwrap().to_owned(), true, None, false).await.unwrap();
    until(&mut rx, |e| e.kind == "state" && e.data["state"] == "idle").await;
    assert!(project.join("live.txt").exists());
}

#[tokio::test]
#[ignore = "uses the real claude; run with SWB_LIVE=1 and --ignored"]
async fn live_claude_calls_the_history_search_tool_after_a_click() {
    if std::env::var_os("SWB_LIVE").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let server = swb_app::server::start(swb_app::server::Config {
        port: 0,
        data_dir: dir.path().join("data"),
        bin: None,
        mcp_bin: Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_smart-workbench-app"))),
    })
    .await
    .unwrap();
    let session = server.registry.create(project.to_str().unwrap(), Some("haiku".into())).unwrap();
    let cwd = session.info().await.unwrap().meta.cwd.to_string_lossy().into_owned();

    // An earlier chat of the same project, put straight into the index.
    let old = Envelope { v: 1, sid: "earlier".into(), seq: 1, ts: 1_793_800_000_000, kind: "user".into(), data: serde_json::json!({"text": "we decided the codeword is PELICAN-NONCE-7731 for the release"}) };
    server.registry.index().unwrap().add(&cwd, 0, &old);
    server.registry.index().unwrap().flush();

    let (_, mut rx) = session.subscribe(0).await.unwrap();
    session.send("Use the search_history tool to look for PELICAN-NONCE and tell me, in one sentence, what the excerpt says the codeword is.".into(), vec![]).await.unwrap();
    let ask = until(&mut rx, |e| e.kind == "permission").await;
    assert_eq!(ask.data["tool_name"], "mcp__swb_app__search_history");
    session.permission(ask.data["request_id"].as_str().unwrap().to_owned(), true, None, true).await.unwrap();

    let mut said = String::new();
    loop {
        let e = until(&mut rx, |_| true).await;
        if e.kind == "claude" && e.data["type"] == "assistant" {
            for block in e.data["message"]["content"].as_array().into_iter().flatten() {
                if let Some(t) = block["text"].as_str() {
                    said.push_str(t);
                }
            }
        }
        if e.kind == "state" && e.data["state"] == "idle" {
            break;
        }
    }
    assert!(said.contains("PELICAN-NONCE-7731"), "Claude's answer should quote the old chat: {said}");
}
