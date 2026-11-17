//! A chat's name (given when it is made, changed later, kept when a message is sent) and deleting a chat for good.
use std::time::Duration;

use tenrec::server::{start, Config};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

#[tokio::test]
async fn a_chat_can_be_named_renamed_and_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("scenario.jsonl"), "{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n{\"$wait\":\"user\"}\n").unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let data = root.join("data");
    let server = start(Config { port: 0, data_dir: data.clone(), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let chat = server.registry.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    let sid = chat.sid.clone();
    let title = || async { chat.info().await.unwrap().meta.title };

    assert!(chat.rename("   ".into()).await.is_err() && chat.rename("\u{1b}\n".into()).await.is_err(), "a blank name is refused");
    chat.rename("  내 작업\u{7} ".into()).await.unwrap();
    assert_eq!(title().await, "내 작업");
    // A first message does not replace a name that was given.
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("hello there".into(), vec![]).await.unwrap();
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.unwrap().unwrap();
        if got.kind == "state" && got.data["state"] == "idle" {
            break;
        }
    }
    assert_eq!(title().await, "내 작업");
    // The name a person gave is also the CLI's own session name (`--name`); a chat nobody named gets none.
    for _ in 0..50 {
        if std::fs::read_to_string(root.join("fake.log")).unwrap_or_default().contains("ARGV") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let argv = std::fs::read_to_string(root.join("fake.log")).unwrap();
    assert!(argv.lines().any(|l| l.starts_with("ARGV ") && l.contains("--name 내 작업")), "{argv}");
    chat.rename("가".repeat(80)).await.unwrap();
    assert_eq!(title().await.chars().count(), 60);

    // Not while a turn runs. (A rename ended the idle process: this message starts a new one.)
    chat.send("again".into(), vec![]).await.unwrap();
    assert!(server.registry.delete(&sid).await.is_err(), "not while a turn runs");
    // Once the turn is over it works (an idle announcement from the rename may arrive first, so it is tried again).
    assert!(root.join("data").join(format!("{sid}.jsonl")).exists());
    let mut deleted = Err(String::new());
    for _ in 0..80 {
        deleted = server.registry.delete(&sid).await;
        if deleted.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    deleted.unwrap();
    assert!(!data.join(format!("{sid}.jsonl")).exists() && !data.join(format!("{sid}.meta.json")).exists());
    assert!(server.registry.get(&sid).is_none() && server.registry.list().await.is_empty());
    assert!(server.registry.delete(&sid).await.is_err(), "it is gone");
    // Nothing is written for it afterwards.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!data.join(format!("{sid}.jsonl")).exists());
}
