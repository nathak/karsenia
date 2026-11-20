//! A model that goes on by itself (a background job ended and the program told it) is shown as running until that turn's result: the
//! chat must not look idle while it speaks.
use std::time::Duration;

use tenrec::server::{start, Config};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

#[tokio::test]
async fn a_turn_the_model_starts_by_itself_makes_the_chat_running_until_its_result() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let text = |t: &str| format!("{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{t}\"}}]}}}}\n");
    let result = "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n";
    // The turn ends, and later the program speaks again with no message of ours (the background job ended).
    let scenario = format!("{{\"$wait\":\"user\"}}\n{}{result}{{\"$sleep\":400}}\n{}{{\"$sleep\":300}}\n{result}", text("started it"), text("the job ended, going on"));
    std::fs::write(root.join("scenario.jsonl"), scenario).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let chat = server.registry.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("go".into(), vec![]).await.unwrap();
    // The states, in order, until the second result: running (our message), idle (its result), running (it spoke by itself), idle.
    let mut states = Vec::new();
    while states.len() < 4 {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.expect("an event in time").unwrap();
        if got.kind == "state" {
            states.push(got.data["state"].as_str().unwrap_or_default().to_owned());
        }
    }
    assert_eq!(states, ["running", "idle", "running", "idle"], "{states:?}");
}
