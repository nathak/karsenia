//! Control requests over the process's stdin: a running idle chat changes its model and permission mode without a restart, a job is
//! stopped, `/model name` typed in the composer is the app's own, and the command names of the program's `init` are kept.
use std::time::Duration;

use tenrec::claude::perm::PermMode;
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
    server: tenrec::server::Running,
    chat: tenrec::session::SessionHandle,
}

const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"S1","cwd":"/p","model":"haiku","slash_commands":["compact","doctor","my-skill","plug:go"],"terminal_slash_commands":["doctor"]}"#;
const DONE: &str = r#"{"type":"result","subtype":"success","is_error":false}"#;

/// A chat whose first turn is done and whose process then follows `after` (directives of the stand-in).
async fn rig(after: &[&str]) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut scenario = format!("{{\"$wait\":\"user\"}}\n{INIT}\n{DONE}\n");
    for line in after {
        scenario.push_str(line);
        scenario.push('\n');
    }
    std::fs::write(root.join("scenario.jsonl"), scenario).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let chat = server.registry.create_in(project.to_str().unwrap(), Some("haiku".into()), None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("one".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    Rig { _dir: dir, root, server, chat }
}

impl Rig {
    fn log(&self) -> String {
        std::fs::read_to_string(self.root.join("fake.log")).unwrap_or_default()
    }
    fn starts(&self) -> usize {
        self.log().lines().filter(|l| l.starts_with("ARGV ")).count()
    }
    async fn meta(&self) -> tenrec::session::Meta {
        self.server.registry.list().await.into_iter().find(|i| i.meta.sid == self.chat.sid).unwrap().meta
    }
}

#[tokio::test]
async fn a_live_model_change_is_a_control_request_and_does_not_restart_the_process() {
    let r = rig(&[r#"{"$reply":"set_model","response":{"model":"claude-opus-5-5"}}"#, r#"{"$sleep":20000}"#]).await;
    r.chat.set_model(Some("opus".into())).await.unwrap();
    assert_eq!(r.meta().await.model.as_deref(), Some("opus"));
    assert_eq!(r.starts(), 1, "{}", r.log());
    let line = r.log().lines().find(|l| l.starts_with("IN ") && l.contains("set_model")).map(str::to_owned).expect("the request was sent");
    let v: serde_json::Value = serde_json::from_str(line.trim_start_matches("IN ")).unwrap();
    assert_eq!((v["type"].as_str(), v["request"]["model"].as_str()), (Some("control_request"), Some("opus")));
    let (replay, _) = r.chat.subscribe(0).await.unwrap();
    assert!(replay.iter().any(|e| e.kind == "model" && e.data["live"] == true && e.data["model"] == "opus"));
    // The answer is the program's own and is not shown as a line of the chat.
    assert!(!replay.iter().any(|e| e.kind == "claude" && e.data["type"] == "control_response"));
}

#[tokio::test]
async fn a_refused_live_change_falls_back_to_ending_the_process_and_resuming_with_the_new_model() {
    let r = rig(&[r#"{"$reply":"set_model","error":"unsupported"}"#, r#"{"$wait":"user"}"#]).await;
    r.chat.set_model(Some("opus".into())).await.unwrap();
    assert_eq!(r.meta().await.model.as_deref(), Some("opus"));
    r.chat.send("two".into(), vec![]).await.unwrap();
    for _ in 0..100 {
        if r.starts() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let log = r.log();
    let second = log.lines().filter(|l| l.starts_with("ARGV ")).nth(1).expect("a second start").to_owned();
    assert!(second.contains("--model opus") && second.contains("--resume S1"), "{second}");
}

#[tokio::test]
async fn typing_slash_model_with_a_name_changes_the_model_and_sends_no_message_to_the_program() {
    let r = rig(&[r#"{"$reply":"set_model","response":{"model":"x"}}"#, r#"{"$sleep":20000}"#]).await;
    r.chat.send("/model haiku-x".into(), vec![]).await.ok();
    // "haiku-x" is not a model the app knows by name: it goes through as text (the program explains), nothing is changed.
    assert_eq!(r.meta().await.model.as_deref(), Some("haiku"));
    let r = rig(&[r#"{"$reply":"set_model","response":{"model":"x"}}"#, r#"{"$sleep":20000}"#]).await;
    r.chat.send("/model sonnet".into(), vec![]).await.unwrap();
    assert_eq!(r.meta().await.model.as_deref(), Some("sonnet"));
    let (replay, _) = r.chat.subscribe(0).await.unwrap();
    assert!(replay.iter().any(|e| e.kind == "user" && e.data["text"] == "/model sonnet"));
    let user_lines = r.log().lines().filter(|l| l.starts_with("IN ") && l.contains("\"type\":\"user\"")).count();
    assert_eq!(user_lines, 1, "only the first message reached the program: {}", r.log());
}

#[tokio::test]
async fn the_permission_mode_changes_live_and_is_kept_for_the_next_process() {
    let r = rig(&[r#"{"$reply":"set_permission_mode","response":{"mode":"plan"}}"#, r#"{"$reply":"set_permission_mode","error":"not allowed"}"#, r#"{"$sleep":20000}"#]).await;
    r.chat.set_permission_mode(PermMode::Plan).await.unwrap();
    assert_eq!(r.meta().await.permission_mode, PermMode::Plan);
    // A refusal by the program leaves the mode as it was and says why.
    let err = r.chat.set_permission_mode(PermMode::AcceptEdits).await.unwrap_err();
    assert!(err.contains("not allowed"), "{err}");
    assert_eq!(r.meta().await.permission_mode, PermMode::Plan);
    // Bypass is only for a chat made with it.
    assert!(r.chat.set_permission_mode(PermMode::Bypass).await.is_err());
    assert_eq!(r.starts(), 1);
}

#[tokio::test]
async fn a_chat_without_a_process_takes_a_mode_for_its_first_start_and_a_job_stop_needs_a_process() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some("/no/such/claude".into()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let chat = server.registry.create_in(project.to_str().unwrap(), None, None, false, None).unwrap();
    chat.set_permission_mode(PermMode::AcceptEdits).await.unwrap();
    let info = server.registry.list().await.into_iter().find(|i| i.meta.sid == chat.sid).unwrap();
    assert_eq!(info.meta.permission_mode, PermMode::AcceptEdits);
    assert!(chat.stop_task("t1".into()).await.is_err());
}

#[tokio::test]
async fn a_task_is_stopped_with_its_id_and_the_commands_of_init_are_kept_without_the_terminal_ones() {
    let r = rig(&[r#"{"$reply":"stop_task","response":{}}"#, r#"{"$sleep":20000}"#]).await;
    r.chat.stop_task("task-7".into()).await.unwrap();
    let line = r.log().lines().find(|l| l.starts_with("IN ") && l.contains("stop_task")).map(str::to_owned).unwrap();
    assert!(line.contains("\"task_id\":\"task-7\""), "{line}");
    assert_eq!(r.meta().await.slash_commands, ["compact", "my-skill", "plug:go"]);
}
