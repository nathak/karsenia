//! Changing the model of a chat between turns: the `claude` process is ended, and the next message starts it again with
//! `--resume <session id> --model <new>` (measured with Claude Code 2.1.325: same session, other model, conversation kept).
use std::time::Duration;

use serde_json::{json, Value};
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

/// The stand-in logs what it reads a moment after the host sent it.
async fn starts_reach(log: &std::path::Path, n: usize) {
    for _ in 0..100 {
        if std::fs::read_to_string(log).unwrap_or_default().lines().filter(|l| l.starts_with("ARGV ")).count() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn the_next_message_after_a_model_change_resumes_the_same_session_with_the_new_model() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let scenario = "{\"$wait\":\"user\"}\n{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"S1\",\"cwd\":\"/p\",\"model\":\"haiku\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n{\"$wait\":\"user\"}\n";
    std::fs::write(root.join("scenario.jsonl"), scenario).unwrap();
    let log = root.join("fake.log");
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), log.display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let chat = server.registry.create_in(project.to_str().unwrap(), Some("haiku".into()), None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();

    // A chat that has not started anything: the model is just changed.
    chat.send("one".into(), vec![]).await.unwrap();
    // While the turn runs it is refused.
    let refused = chat.set_model(Some("sonnet".into())).await;
    idle(&mut rx).await;
    assert!(refused.is_err(), "not while a turn runs");
    assert!(chat.set_model(Some("sonnet".into())).await.is_ok());
    // No "exited" state is announced for the process that was ended on purpose; the next message works.
    chat.send("two".into(), vec![]).await.unwrap();
    starts_reach(&log, 2).await;
    let text = std::fs::read_to_string(&log).unwrap();
    let starts: Vec<&str> = text.lines().filter(|l| l.starts_with("ARGV ")).collect();
    assert_eq!(starts.len(), 2, "{text}");
    assert!(starts[0].contains("--model haiku") && !starts[0].contains("--resume"), "{}", starts[0]);
    assert!(starts[1].contains("--model sonnet") && starts[1].contains("--resume S1"), "{}", starts[1]);
    let meta: Value = serde_json::from_str(&std::fs::read_to_string(root.join("data").join(format!("{}.meta.json", chat.sid))).unwrap()).unwrap();
    assert_eq!(meta["model"], json!("sonnet"));

    // The default is no `--model` at all; the same model again changes nothing.
    assert!(chat.set_model(Some("sonnet".into())).await.is_ok());
    assert!(chat.set_model(None).await.is_ok());
    chat.send("three".into(), vec![]).await.unwrap();
    starts_reach(&log, 3).await;
    let text = std::fs::read_to_string(&log).unwrap();
    let last = text.lines().filter(|l| l.starts_with("ARGV ")).next_back().unwrap().to_owned();
    assert!(!last.contains("--model") && last.contains("--resume S1"), "{last}");
    chat.stop();
}

#[test]
fn only_a_plain_model_name_is_taken() {
    use tenrec::registry::clean_model;
    for ok in ["sonnet", "opus[1m]", "claude-sonnet-5-5", " haiku "] {
        assert!(clean_model(ok).is_some(), "{ok}");
    }
    assert_eq!(clean_model(" haiku ").as_deref(), Some("haiku"));
    for bad in ["", "  ", "default", "a b", "x;rm", "--model", &"m".repeat(101)] {
        assert!(clean_model(bad).is_none() || bad == "--model", "{bad}");
    }
}
