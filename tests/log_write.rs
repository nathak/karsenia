//! A chat whose log cannot be written says so once (it used to lose the writes without a word) and goes on working.
use std::time::Duration;

use tenrec::server::{start, Config};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

#[tokio::test]
async fn a_log_that_cannot_be_written_is_told_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let turn = "{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n";
    std::fs::write(root.join("scenario.jsonl"), format!("{turn}{turn}")).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: Some(wrapper.clone()), extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let chat = server.registry.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    // A folder where the log file is: opening it for appending fails (whoever runs this).
    let log = tenrec::session::log_path(&root.join("data"), &chat.sid);
    let _ = std::fs::remove_file(&log);
    std::fs::create_dir(&log).unwrap();

    for text in ["one", "two"] {
        chat.send(text.into(), vec![]).await.unwrap();
        loop {
            let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.expect("the chat went on").unwrap();
            if got.kind == "state" && got.data["state"] == "idle" {
                break;
            }
        }
    }
    let (replay, _) = chat.subscribe(0).await.unwrap();
    let told = replay.iter().filter(|e| e.kind == "error" && e.data["message"].as_str().is_some_and(|m| m.contains("could not be written"))).count();
    assert_eq!(told, 1, "told once, not once per event");
    assert_eq!(replay.iter().filter(|e| e.kind == "user").count(), 2, "the chat kept working in memory");
}
