//! A chat after the app was started again: its name, its conversation and the Claude session it resumes are what they were.
use std::time::Duration;

use tenrec::server::{start, Config};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");
const ID: &str = "748aa02c-6299-4224-8d60-1131a940bf14";

async fn idle(rx: &mut tokio::sync::broadcast::Receiver<tenrec::session::Envelope>) {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.unwrap().unwrap();
        if got.kind == "state" && got.data["state"] == "idle" {
            return;
        }
    }
}

#[tokio::test]
async fn a_renamed_chat_keeps_its_name_and_history_across_a_restart_and_a_new_message() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let scenario = format!("{{\"$wait\":\"user\"}}\n{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"{ID}\"}}\n{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}}\n{{\"$wait\":\"user\"}}\n{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"{ID}\"}}\n{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}}\n{{\"$wait\":\"user\"}}\n");
    std::fs::write(root.join("scenario.jsonl"), scenario).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let config = || Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() };

    let sid = {
        let server = start(config()).await.unwrap();
        let chat = server.registry.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
        let (_, mut rx) = chat.subscribe(0).await.unwrap();
        chat.send("first question".into(), vec![]).await.unwrap();
        idle(&mut rx).await;
        chat.rename("My name".into()).await.unwrap();
        let sid = chat.sid.clone();
        chat.stop();
        drop(server);
        sid
    };

    // The app is started again.
    let server = start(config()).await.unwrap();
    let chat = server.registry.get(&sid).expect("the chat is listed again");
    let info = chat.info().await.unwrap();
    assert_eq!(info.meta.title, "My name");
    assert!(info.meta.named);
    assert_eq!(info.meta.claude_session_id.as_deref(), Some(ID));
    // Its conversation is there without any new message.
    let (replay, mut rx) = chat.subscribe(0).await.unwrap();
    assert!(replay.iter().any(|e| e.kind == "user" && e.data["text"] == "first question"), "the old conversation is replayed");
    assert!(replay.iter().any(|e| e.kind == "renamed" && e.data["title"] == "My name"));

    // A new message resumes the session and keeps the name.
    chat.send("second question".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    assert_eq!(chat.info().await.unwrap().meta.title, "My name");
    let again = std::fs::read_to_string(root.join("data").join(format!("{sid}.meta.json"))).unwrap();
    assert!(again.contains("My name"), "{again}");
    for _ in 0..50 {
        if std::fs::read_to_string(root.join("fake.log")).unwrap_or_default().matches("ARGV").count() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let argv = std::fs::read_to_string(root.join("fake.log")).unwrap();
    assert!(argv.lines().filter(|l| l.starts_with("ARGV ")).last().is_some_and(|l| l.contains(&format!("--resume {ID}")) && l.contains("--name My name")), "{argv}");
}

#[tokio::test]
async fn a_chat_named_but_never_talked_to_can_be_sent_to_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("scenario.jsonl"), format!("{{\"$wait\":\"user\"}}\n{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"{ID}\"}}\n{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}}\n{{\"$wait\":\"user\"}}\n")).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let config = || Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() };

    let sid = {
        let server = start(config()).await.unwrap();
        let chat = server.registry.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
        chat.rename("test".into()).await.unwrap();
        let sid = chat.sid.clone();
        chat.stop();
        drop(server);
        sid
    };
    let server = start(config()).await.unwrap();
    let chat = server.registry.get(&sid).expect("the chat is found again");
    assert_eq!(chat.info().await.unwrap().meta.title, "test");
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("hello".into(), vec![]).await.expect("a message can be sent to it");
    idle(&mut rx).await;
}
