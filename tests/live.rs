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
    session.permission(ask.data["request_id"].as_str().unwrap().to_owned(), true, None).await.unwrap();
    until(&mut rx, |e| e.kind == "state" && e.data["state"] == "idle").await;
    assert!(project.join("live.txt").exists());
}
