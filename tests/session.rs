//! The session task against a fake `claude`: permissions wait for a person, answers carry the request's own input,
//! a process that dies cancels what waited, a conversation resumes by the CLI's session id.
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use swb_app::claude::protocol::Image;
use swb_app::registry::Registry;
use swb_app::session::{Envelope, SessionHandle};
use tempfile::TempDir;
use tokio::sync::broadcast::Receiver;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

struct Fx {
    _dir: TempDir,
    root: PathBuf,
    registry: Registry,
    log: PathBuf,
    project: PathBuf,
}

fn fixture(scenario: &str) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let scenario_file = root.join("scenario.jsonl");
    std::fs::write(&scenario_file, scenario).unwrap();
    let log = root.join("fake.log");
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", scenario_file.display(), log.display(), FAKE)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let registry = Registry::open(root.join("data"), Some(wrapper.display().to_string())).unwrap();

    Fx { _dir: dir, root, registry, log, project }
}

impl Fx {
    fn session(&self) -> SessionHandle {
        self.registry.create(self.project.to_str().unwrap(), Some("haiku".into())).unwrap()
    }

    fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

async fn until(rx: &mut Receiver<Envelope>, what: impl Fn(&Envelope) -> bool) -> Envelope {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.expect("timed out waiting for an event").expect("the stream ended");
        if what(&got) {
            return got;
        }
    }
}

fn kind(k: &'static str) -> impl Fn(&Envelope) -> bool {
    move |e| e.kind == k
}

fn state(s: &'static str) -> impl Fn(&Envelope) -> bool {
    move |e| e.kind == "state" && e.data["state"] == s
}

const ASK: &str = r#"{"$wait":"user"}
{"type":"system","subtype":"init","session_id":"S1","cwd":"/p","model":"haiku"}
{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"touch a"},"permission_suggestions":[]}}
{"$wait":"control_response"}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"done"}]}}
{"type":"result","subtype":"success","is_error":false}
{"$wait":"user"}
"#;

#[tokio::test]
async fn a_permission_waits_for_a_person_and_the_answer_carries_the_requests_own_input() {
    let fx = fixture(ASK);
    let session = fx.session();
    let (_, mut rx) = session.subscribe(0).await.unwrap();
    session.send("go".into(), vec![]).await.unwrap();
    let ask = until(&mut rx, kind("permission")).await;
    assert_eq!(ask.data["request_id"], "r1");

    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(!fx.log_text().contains("control_response"), "nothing may be answered before a person does");
    assert_eq!(session.info().await.unwrap().pending, 1);

    session.permission("r1".into(), true, None, false).await.unwrap();
    until(&mut rx, state("idle")).await;
    let log = fx.log_text();
    let reply = log.lines().find(|l| l.starts_with("IN ") && l.contains("control_response")).expect("the allow was written");
    let v: Value = serde_json::from_str(&reply[3..]).unwrap();
    assert_eq!(v["response"]["response"]["behavior"], "allow");
    assert_eq!(v["response"]["response"]["updatedInput"]["command"], "touch a");
    assert!(session.permission("r1".into(), true, None, false).await.is_err(), "a request is answered once");
}

#[tokio::test]
async fn a_denial_goes_back_with_its_message() {
    let fx = fixture(ASK);
    let session = fx.session();
    let (_, mut rx) = session.subscribe(0).await.unwrap();
    session.send("go".into(), vec![]).await.unwrap();
    until(&mut rx, kind("permission")).await;
    session.permission("r1".into(), false, Some("not now".into()), false).await.unwrap();
    let resolved = until(&mut rx, kind("permission_resolved")).await;
    assert_eq!(resolved.data["outcome"], "denied");
    until(&mut rx, state("idle")).await;
    assert!(fx.log_text().contains(r#""behavior":"deny""#) && fx.log_text().contains("not now"));
}

#[tokio::test]
async fn an_interrupt_is_sent_while_a_turn_runs() {
    let fx = fixture(
        r#"{"$wait":"user"}
{"type":"system","subtype":"init","session_id":"S2"}
{"$wait":"interrupt"}
{"type":"control_response","response":{"subtype":"success","request_id":"x","response":{"still_queued":[]}}}
{"type":"result","subtype":"error_during_execution","is_error":true}
{"$wait":"user"}
"#,
    );
    let session = fx.session();
    assert!(session.interrupt().await.is_err(), "nothing runs yet");
    let (_, mut rx) = session.subscribe(0).await.unwrap();
    session.send("long".into(), vec![]).await.unwrap();
    until(&mut rx, state("running")).await;
    session.interrupt().await.unwrap();
    until(&mut rx, state("idle")).await;
    assert!(fx.log_text().contains(r#""subtype":"interrupt""#));
}

#[tokio::test]
async fn a_second_message_during_a_turn_is_refused() {
    let fx = fixture("{\"$wait\":\"user\"}\n{\"$sleep\":1500}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n{\"$wait\":\"user\"}\n");
    let session = fx.session();
    session.send("one".into(), vec![]).await.unwrap();
    assert!(session.send("two".into(), vec![]).await.is_err());
}

#[tokio::test]
async fn a_conversation_resumes_by_the_cli_session_id_after_the_process_ends() {
    let fx = fixture(
        r#"{"$wait":"user"}
{"type":"system","subtype":"init","session_id":"S9"}
{"type":"result","subtype":"success","is_error":false}
{"$exit":0}
"#,
    );
    let session = fx.session();
    let (_, mut rx) = session.subscribe(0).await.unwrap();
    session.send("a".into(), vec![]).await.unwrap();
    until(&mut rx, state("exited")).await;
    session.send("b".into(), vec![]).await.unwrap();
    until(&mut rx, state("exited")).await;
    let log = fx.log_text();
    let argv: Vec<&str> = log.lines().filter(|l| l.starts_with("ARGV")).collect();
    assert_eq!(argv.len(), 2);
    assert!(!argv[0].contains("--resume"));
    assert!(argv[1].contains("--resume S9"), "{}", argv[1]);
    assert!(argv[1].contains("--permission-prompt-tool stdio"));
}

#[tokio::test]
async fn a_process_that_dies_cancels_what_waited() {
    let fx = fixture(
        r#"{"$wait":"user"}
{"type":"system","subtype":"init","session_id":"S3"}
{"type":"control_request","request_id":"r7","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"x"}}}
{"$sleep":200}
{"$exit":3}
"#,
    );
    let session = fx.session();
    let (_, mut rx) = session.subscribe(0).await.unwrap();
    session.send("go".into(), vec![]).await.unwrap();
    until(&mut rx, kind("permission")).await;
    let gone = until(&mut rx, kind("permission_resolved")).await;
    assert_eq!(gone.data["outcome"], "cancelled");
    let exited = until(&mut rx, state("exited")).await;
    assert_eq!(exited.data["code"], 3);
    assert!(session.permission("r7".into(), true, None, false).await.is_err());
}

#[tokio::test]
async fn images_are_checked_and_the_log_is_replayed_after_a_restart() {
    let fx = fixture("{\"$wait\":\"user\"}\n{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"S5\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n{\"$wait\":\"user\"}\n");
    let session = fx.session();
    let bad = Image { media_type: "image/svg+xml".into(), data: "AAAA".into() };
    assert!(session.send("x".into(), vec![bad]).await.is_err());
    let huge = Image { media_type: "image/png".into(), data: "A".repeat(7 * 1024 * 1024) };
    assert!(session.send("x".into(), vec![huge]).await.is_err());
    assert!(session.send("   ".into(), vec![]).await.is_err());

    let (_, mut rx) = session.subscribe(0).await.unwrap();
    let ok = Image { media_type: "image/png".into(), data: "iVBORw0KGgo=".into() };
    session.send("look at this".into(), vec![ok]).await.unwrap();
    until(&mut rx, state("idle")).await;
    let (all, _) = session.subscribe(0).await.unwrap();
    let last = all.last().unwrap().seq;
    let (later, _) = session.subscribe(last - 1).await.unwrap();
    assert_eq!(later.len(), 1, "only what came after the given number is replayed");
    assert!(fx.log_text().contains(r#""type":"image""#));

    // A new registry on the same folder finds the session, its title and its log.
    let reopened = Registry::open(fx.root.join("data"), None).unwrap();
    let list = reopened.list().await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].meta.title, "look at this");
    assert_eq!(list[0].meta.claude_session_id.as_deref(), Some("S5"));
    let (again, _) = reopened.get(&session.sid).unwrap().subscribe(0).await.unwrap();
    assert_eq!(again.len(), all.len());
}

#[tokio::test]
async fn a_folder_that_is_not_safe_is_refused() {
    let fx = fixture("");
    assert!(fx.registry.create("/", None).is_err());
    assert!(fx.registry.create("relative/dir", None).is_err());
    assert!(fx.registry.create("/no/such/dir/at/all", None).is_err());
}

#[tokio::test]
async fn a_session_read_back_after_a_crash_has_nothing_left_open() {
    let fx = fixture("");
    let data = fx.root.join("data");
    let meta = serde_json::json!({"sid": "dead01", "cwd": fx.project, "model": null, "claude_session_id": "C1", "title": "x", "created": 1});
    std::fs::write(data.join("dead01.meta.json"), meta.to_string()).unwrap();
    let env = |seq: u64, kind: &str, data: serde_json::Value| serde_json::json!({"v": 1, "sid": "dead01", "seq": seq, "kind": kind, "data": data}).to_string();
    let log = [
        env(1, "state", serde_json::json!({"state": "running"})),
        env(2, "permission", serde_json::json!({"request_id": "old", "tool_name": "Bash", "input": {}})),
    ]
    .join("\n");
    std::fs::write(data.join("dead01.jsonl"), format!("{log}\n")).unwrap();

    let reopened = Registry::open(data, None).unwrap();
    let session = reopened.get("dead01").unwrap();
    let info = session.info().await.unwrap();
    assert_eq!(info.pending, 0);
    assert_eq!(serde_json::to_value(&info.state).unwrap()["state"], "idle");
    let (all, _) = session.subscribe(0).await.unwrap();
    let tail: Vec<(&str, String)> = all.iter().skip(2).map(|e| (e.kind.as_str(), e.data.to_string())).collect();
    assert_eq!(tail.len(), 2, "{tail:?}");
    assert_eq!(tail[0].0, "permission_resolved");
    assert!(tail[0].1.contains("cancelled") && tail[0].1.contains("old"));
    assert_eq!(tail[1].0, "state");
    assert!(tail[1].1.contains("idle"));
}

#[tokio::test]
async fn the_end_of_a_turn_cancels_a_request_nobody_answered() {
    let fx = fixture(
        r#"{"$wait":"user"}
{"type":"system","subtype":"init","session_id":"S6"}
{"type":"control_request","request_id":"r9","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"x"}}}
{"$sleep":300}
{"type":"result","subtype":"error_during_execution","is_error":true}
{"$wait":"user"}
"#,
    );
    let session = fx.session();
    let (_, mut rx) = session.subscribe(0).await.unwrap();
    session.send("go".into(), vec![]).await.unwrap();
    until(&mut rx, kind("permission")).await;
    let gone = until(&mut rx, kind("permission_resolved")).await;
    assert_eq!(gone.data["outcome"], "cancelled");
    until(&mut rx, state("idle")).await;
    assert!(session.permission("r9".into(), true, None, false).await.is_err(), "a dead request is not answered");
    assert!(!fx.log_text().contains("control_response"));
}

#[tokio::test]
async fn streaming_pieces_reach_the_page_but_are_not_kept() {
    let fx = fixture(
        r#"{"$wait":"user"}
{"type":"system","subtype":"init","session_id":"S7"}
{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"PIECE"}}}
{"type":"result","subtype":"success","is_error":false}
{"$wait":"user"}
"#,
    );
    let session = fx.session();
    let (_, mut rx) = session.subscribe(0).await.unwrap();
    session.send("go".into(), vec![]).await.unwrap();
    let piece = until(&mut rx, |e| e.kind == "claude" && e.data["type"] == "stream_event").await;
    assert_eq!(piece.data["event"]["delta"]["text"], "PIECE");
    until(&mut rx, state("idle")).await;
    let (kept, _) = session.subscribe(0).await.unwrap();
    assert!(kept.iter().all(|e| e.data["type"] != "stream_event"), "not in memory");
    let disk = std::fs::read_to_string(fx.root.join("data").join(format!("{}.jsonl", session.sid))).unwrap();
    assert!(!disk.contains("PIECE"), "not on disk");
}

#[tokio::test]
async fn at_most_five_images_and_the_host_can_end_every_process_at_once() {
    let fx = fixture("{\"$wait\":\"user\"}\n{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"S8\"}\n{\"$sleep\":30000}\n");
    let session = fx.session();
    let six: Vec<Image> = (0..6).map(|_| Image { media_type: "image/png".into(), data: "iVBORw0KGgo=".into() }).collect();
    assert!(session.send("x".into(), six).await.is_err());

    let (_, mut rx) = session.subscribe(0).await.unwrap();
    session.send("go".into(), vec![]).await.unwrap();
    until(&mut rx, |e| e.kind == "claude" && e.data["subtype"] == "init").await;
    fx.registry.kill_all();
    let ended = until(&mut rx, |e| e.kind == "state" && e.data["state"] == "exited").await;
    assert!(ended.data["code"].is_null(), "ended by a signal, no exit code");
}
