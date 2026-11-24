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

/// A frame of a sub-agent (it names its parent call) after the turn ended is not the model going on: the chat stays idle.
#[tokio::test]
async fn a_sub_agent_frame_after_the_result_does_not_make_the_chat_running() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let main = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}\n";
    let sub = "{\"type\":\"assistant\",\"parent_tool_use_id\":\"toolu_x\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"a sub-agent speaks\"}]}}\n";
    let result = "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n";
    std::fs::write(root.join("scenario.jsonl"), format!("{{\"$wait\":\"user\"}}\n{main}{result}{{\"$sleep\":300}}\n{sub}{{\"$sleep\":1500}}\n")).unwrap();
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
    let mut states = Vec::new();
    let mut spoke = false;
    // Until the sub-agent's frame has been seen and a moment more has passed.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(900), rx.recv()).await {
            Ok(Ok(got)) => {
                if got.kind == "state" {
                    states.push(got.data["state"].as_str().unwrap_or_default().to_owned());
                }
                if got.kind == "claude" && got.data.to_string().contains("a sub-agent speaks") {
                    spoke = true;
                }
            }
            _ if spoke => break,
            _ => {}
        }
    }
    assert!(spoke, "the sub-agent's frame arrived");
    assert_eq!(states, ["running", "idle"], "{states:?}");
}

/// Ending the process to start it again (here a model change the program refused to take live) ends its background jobs: the list is cleared and the transcript says so.
#[tokio::test]
async fn restarting_the_process_says_that_its_background_jobs_ended() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let jobs = "{\"type\":\"system\",\"subtype\":\"background_tasks_changed\",\"tasks\":[{\"task_id\":\"a\",\"task_type\":\"local_bash\",\"description\":\"make\"}]}\n";
    let result = "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n";
    std::fs::write(root.join("scenario.jsonl"), format!("{{\"$wait\":\"user\"}}\n{jobs}{result}{{\"$reply\":\"set_model\",\"error\":\"unsupported\"}}\n{{\"$sleep\":20000}}\n")).unwrap();
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
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.expect("an event in time").unwrap();
        if got.kind == "state" && got.data["state"] == "idle" {
            break;
        }
    }
    chat.set_model(Some("haiku".into())).await.unwrap();
    let (replay, _) = chat.subscribe(0).await.unwrap();
    let lost = replay.iter().find(|e| e.kind == "bg_lost").expect("the loss is said");
    assert_eq!(lost.data["n"], 1);
    let last = replay.iter().rev().find(|e| e.kind == "claude" && e.data["subtype"] == "background_tasks_changed").unwrap();
    assert!(last.data["tasks"].as_array().unwrap().is_empty(), "{:?}", last.data);
    // Said once: a second restart has nothing more to say.
    chat.set_model(Some("opus".into())).await.unwrap();
    let (again, _) = chat.subscribe(0).await.unwrap();
    assert_eq!(again.iter().filter(|e| e.kind == "bg_lost").count(), 1);
}
