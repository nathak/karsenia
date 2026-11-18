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
    let work = server.registry.add_account("Work", tenrec::accounts::AccountKind::Claude).unwrap().id;

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

    // The guard and the contract of the chat: strict, paused, a locked contract and a pin.
    use tenrec::workbench::{Op, Profile};
    server.registry.workbench_apply(&chat.sid, Op::SetProfile { profile: Profile::Strict }).unwrap();
    server.registry.workbench_apply(&chat.sid, Op::SetPaused { paused: true }).unwrap();
    server.registry.workbench_apply(&chat.sid, Op::SetGoal { text: "ship".into() }).unwrap();
    server.registry.workbench_apply(&chat.sid, Op::PinNote { text: "remember".into() }).unwrap();
    server.registry.workbench_apply(&chat.sid, Op::Lock).unwrap();
    let moved = server.registry.continue_as(&chat.sid, Some(&work)).await.unwrap();
    let carried = server.registry.workbench_get(&moved.sid).unwrap();
    assert_eq!((carried.profile, carried.paused, carried.task.locked, carried.task.goal.as_str(), carried.pins.len()), (Profile::Strict, true, true, "ship", 1), "the guard and the contract come along");
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

    // The chat in the other account goes on: a line is added to its session file (what Claude Code does) and its own guard changes.
    moved.stop();
    let work_file = tenrec::handover::session_path(&work_cfg, &cwd, ID);
    let mut text = std::fs::read_to_string(&work_file).unwrap();
    text.push_str("{\"type\":\"user\",\"message\":{\"content\":\"asked in the other account\"}}\n");
    std::fs::write(&work_file, &text).unwrap();
    server.registry.workbench_apply(&moved.sid, Op::SetProfile { profile: Profile::Permissive }).unwrap();

    // Back to the default account: its first chat is brought up to date and reopened; no second chat is made there.
    let before = server.registry.list().await.len();
    let back = server.registry.continue_as(&moved.sid, None).await.unwrap();
    assert_eq!(back.sid, chat.sid, "the original chat comes back");
    assert_eq!(server.registry.list().await.len(), before, "no chat was added");
    assert!(std::fs::read_to_string(&src).unwrap().contains("asked in the other account") && !std::fs::read_to_string(&src).unwrap().contains("session-owner"), "its session file is the newer one");
    let (replay, _) = back.subscribe(0).await.unwrap();
    assert!(replay.iter().any(|e| e.kind == "user" && e.data["text"] == "what was the word" && e.data["imported"] == true), "what was said in the other account is in the chat");
    assert_eq!(server.registry.workbench_get(&chat.sid).unwrap().profile, Profile::Permissive, "the guard comes back with it");
    // And on to the other account again: its chat is brought up to date the same way.
    let again = server.registry.continue_as(&chat.sid, Some(&work)).await.unwrap();
    assert_eq!(again.sid, moved.sid);

    // Only an unchanged older chat is brought up to date: a session file that has changed since is never replaced.
    let mut changed = std::fs::read_to_string(&src).unwrap();
    changed.push_str("{\"type\":\"user\",\"message\":{\"content\":\"added at home\"}}\n");
    std::fs::write(&src, changed).unwrap();
    let refused = server.registry.continue_as(&moved.sid, None).await.err().unwrap_or_default();
    assert!(refused.contains("has changed since"), "{refused}");
    assert!(std::fs::read_to_string(&src).unwrap().contains("added at home"));
    // A chat that is not an earlier state of this one is never replaced either, even after its chat in between was deleted.
    server.registry.delete(&moved.sid).await.unwrap();
    let after = server.registry.continue_as(&chat.sid, Some(&work)).await.err().unwrap_or_default();
    assert!(after.contains("already has this session"), "{after}");
}
