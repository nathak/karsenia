//! Archiving a chat: hidden from the list instead of deleted, kept across a restart, never while a turn runs or a question waits, and
//! undone by sending to it.
use std::time::Duration;

use tenrec::server::{start, Config};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

async fn idle(rx: &mut tokio::sync::broadcast::Receiver<tenrec::session::Envelope>) {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.expect("timed out").unwrap();
        if got.kind == "state" && got.data["state"] == "idle" {
            return;
        }
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    project: std::path::PathBuf,
    wrapper: std::path::PathBuf,
}

/// `scenario`: what the fake `claude` does (each `$wait` is a message it waits for).
fn rig(scenario: &str) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("scenario.jsonl"), scenario).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    Rig { _dir: dir, root, project, wrapper }
}

impl Rig {
    fn config(&self) -> Config {
        Config { port: 0, data_dir: self.root.join("data"), bin: Some(self.wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }
    }
}

const TURN: &str = "{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n";

#[tokio::test]
async fn archive_persists_across_a_restart_and_unarchive_clears_it_and_the_log_is_kept() {
    let r = rig(&format!("{TURN}{TURN}"));
    let sid = {
        let server = start(r.config()).await.unwrap();
        let chat = server.registry.create_in(r.project.to_str().unwrap(), None, None, false, None).unwrap();
        let (_, mut rx) = chat.subscribe(0).await.unwrap();
        chat.send("a question".into(), vec![]).await.unwrap();
        idle(&mut rx).await;
        assert!(!chat.info().await.unwrap().meta.archived);
        chat.set_archived(true).await.unwrap();
        assert!(chat.info().await.unwrap().meta.archived);
        let sid = chat.sid.clone();
        chat.stop();
        drop(server);
        sid
    };
    let server = start(r.config()).await.unwrap();
    let chat = server.registry.get(&sid).expect("an archived chat is still listed by the core");
    assert!(chat.info().await.unwrap().meta.archived, "archived across a restart");
    let (replay, _) = chat.subscribe(0).await.unwrap();
    assert!(replay.iter().any(|e| e.kind == "user" && e.data["text"] == "a question"), "its conversation is kept");
    chat.set_archived(false).await.unwrap();
    assert!(!chat.info().await.unwrap().meta.archived);
    assert!(!std::fs::read_to_string(std::fs::read_dir(r.root.join("data")).unwrap().flatten().map(|e| e.path()).find(|p| p.is_dir() && p.join(format!("{sid}.meta.json")).exists()).map(|p| p.join(format!("{sid}.meta.json"))).unwrap_or_else(|| r.root.join("data").join("none"))).unwrap_or_default().contains("archived"), "an unarchived chat writes no flag");
}

#[tokio::test]
async fn archive_is_refused_while_a_turn_runs() {
    // The fake waits for a message and then goes on running (it never answers within the test).
    let r = rig("{\"$wait\":\"user\"}\n{\"$sleep\":5000}\n");
    let server = start(r.config()).await.unwrap();
    let chat = server.registry.create_in(r.project.to_str().unwrap(), None, None, false, None).unwrap();
    chat.send("go".into(), vec![]).await.unwrap();
    let err = chat.set_archived(true).await.unwrap_err();
    assert!(err.contains("turn") || err.contains("running") || err.contains("waits"), "{err}");
    assert!(!chat.info().await.unwrap().meta.archived);
    chat.stop();
}

#[tokio::test]
async fn sending_to_an_archived_chat_brings_it_back() {
    let r = rig(&format!("{TURN}{TURN}"));
    let server = start(r.config()).await.unwrap();
    let chat = server.registry.create_in(r.project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("one".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    chat.set_archived(true).await.unwrap();
    chat.send("two".into(), vec![]).await.unwrap();
    assert!(!chat.info().await.unwrap().meta.archived, "a chat that is being used is not archived");
    idle(&mut rx).await;
    chat.stop();
}
