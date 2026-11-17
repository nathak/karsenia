//! A chat carried on under another account: the same conversation, the Claude session copied to the other account's folder and resumed
//! there. It sets the default account's folder through the environment, so it is the only test in its file.
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
async fn a_chat_continues_in_another_account_with_its_conversation_and_session() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let default_cfg = root.join("default-claude");
    std::fs::create_dir_all(&default_cfg).unwrap();
    // SAFETY: the only test in this file, and nothing else runs yet.
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &default_cfg) };
    let scenario = format!("{{\"$wait\":\"user\"}}\n{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"{ID}\"}}\n{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}}\n{{\"$wait\":\"user\"}}\n{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}}\n{{\"$wait\":\"user\"}}\n");
    std::fs::write(root.join("scenario.jsonl"), scenario).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' FAKE_ENV_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), root.join("fake.log").display(), root.join("env.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let work = server.registry.add_account("Work").unwrap().id;

    // A chat of the default account has one turn.
    let chat = server.registry.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("remember the word kiwi".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    let cwd = chat.info().await.unwrap().meta.cwd.to_string_lossy().into_owned();
    assert_eq!(chat.info().await.unwrap().meta.claude_session_id.as_deref(), Some(ID));
    // What Claude Code keeps for it (the fake does not write one).
    let src = tenrec::handover::session_path(&default_cfg, &cwd, ID);
    std::fs::create_dir_all(src.parent().unwrap()).unwrap();
    std::fs::write(&src, format!("{{\"type\":\"session-owner\",\"accountUuid\":\"x\",\"sessionId\":\"{ID}\"}}\n{{\"type\":\"user\",\"message\":{{\"content\":\"remember the word kiwi\"}}}}\n")).unwrap();

    // Refused: the same account, an unknown chat.
    assert!(server.registry.continue_as(&chat.sid, None).await.is_err(), "already in that account");
    assert!(server.registry.continue_as("nope", Some(&work)).await.is_err());

    let moved = server.registry.continue_as(&chat.sid, Some(&work)).await.unwrap();
    assert_ne!(moved.sid, chat.sid, "a new chat is made; the old one stays");
    let info = moved.info().await.unwrap();
    assert_eq!(info.meta.account.as_ref().map(|a| a.id.as_str()), Some(work.as_str()));
    assert_eq!(info.meta.claude_session_id.as_deref(), Some(ID));
    assert!(chat.info().await.unwrap().meta.account.is_none(), "the old chat is untouched");
    // The session is in the other account's folder, without the owner line.
    let (_, work_cfg) = (0, server.registry.account_login(&work).unwrap().0);
    let copied = std::fs::read_to_string(tenrec::handover::session_path(&work_cfg, &cwd, ID)).unwrap();
    assert!(copied.contains("kiwi") && !copied.contains("session-owner"), "{copied}");
    // The conversation shows in the new chat.
    let (replay, mut rx2) = moved.subscribe(0).await.unwrap();
    assert!(replay.iter().any(|e| e.kind == "user" && e.data["text"] == "remember the word kiwi" && e.data["imported"] == true));

    // The next message resumes that session as the other account.
    moved.send("what was the word".into(), vec![]).await.unwrap();
    idle(&mut rx2).await;
    for _ in 0..50 {
        if std::fs::read_to_string(root.join("fake.log")).unwrap_or_default().matches("ARGV").count() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let argv = std::fs::read_to_string(root.join("fake.log")).unwrap();
    assert!(argv.lines().filter(|l| l.starts_with("ARGV ")).any(|l| l.contains(&format!("--resume {ID}"))), "{argv}");
    let env = std::fs::read_to_string(root.join("env.log")).unwrap();
    assert!(env.contains(work_cfg.to_str().unwrap()), "the account's own folder is the one the CLI runs with: {env}");

    // Back to the default account is refused: its first chat still carries this session, with its own turns.
    moved.stop();
    let back = server.registry.continue_as(&moved.sid, None).await.err().unwrap_or_default();
    assert!(back.contains("already has a chat"), "{back}");
    // And a second carry to the same account is refused the same way.
    let again = server.registry.continue_as(&chat.sid, Some(&work)).await.err().unwrap_or_default();
    assert!(again.contains("already has a chat"), "{again}");
}
