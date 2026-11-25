//! A conversation the program cannot resume (its session file is gone: it says so and exits before any init) is told to the person, and
//! the next message starts a new conversation instead of failing the same way for good.
use std::time::Duration;

use tenrec::server::{start, Config};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

async fn until_state(rx: &mut tokio::sync::broadcast::Receiver<tenrec::session::Envelope>, wanted: &str) {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.unwrap_or_else(|_| panic!("timed out waiting for {wanted}")).unwrap();
        if got.kind == "state" && got.data["state"] == wanted {
            return;
        }
    }
}

#[tokio::test]
async fn a_conversation_that_cannot_be_resumed_is_told_and_the_next_message_starts_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let init = r#"{"type":"system","subtype":"init","session_id":"S1","cwd":"/p","model":"haiku"}"#;
    let done = r#"{"type":"result","subtype":"success","is_error":false}"#;
    std::fs::write(root.join("scenario.jsonl"), format!("{{\"$wait\":\"user\"}}\n{init}\n{done}\n{{\"$exit\":0}}\n")).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\necho \"WRAP $*\" >> '{wrap}'\ncase \" $* \" in *\" --resume \"*) if [ ! -e '{flag}' ]; then touch '{flag}'; echo 'No conversation found with session ID: S1' >&2; exit 1; fi;; esac\nFAKE_SCENARIO='{scenario}' FAKE_LOG='{log}' exec '{fake}' \"$@\"\n",
            wrap = root.join("wrap.log").display(),
            flag = root.join("resumed-once").display(),
            scenario = root.join("scenario.jsonl").display(),
            log = root.join("fake.log").display(),
            fake = FAKE
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let chat = server.registry.create_in(project.to_str().unwrap(), Some("haiku".into()), None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();

    // A first turn gives the chat its conversation (S1); the program then ends.
    chat.send("one".into(), vec![]).await.unwrap();
    until_state(&mut rx, "exited").await;
    let meta = || async { server.registry.list().await.into_iter().find(|i| i.meta.sid == chat.sid).unwrap().meta };
    assert_eq!(meta().await.claude_session_id.as_deref(), Some("S1"));

    // The next message resumes S1, which the program no longer has: it says so and exits with 1.
    chat.send("two".into(), vec![]).await.unwrap();
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.expect("the person is told the conversation is lost").unwrap();
        if got.kind == "error" && got.data["resume_lost"] == "S1" {
            break;
        }
    }
    until_state(&mut rx, "exited").await;
    assert_eq!(meta().await.claude_session_id, None, "the lost conversation is forgotten, also on disk");

    // The one after that starts a new conversation, without --resume.
    chat.send("three".into(), vec![]).await.unwrap();
    until_state(&mut rx, "idle").await;
    let wrap = std::fs::read_to_string(root.join("wrap.log")).unwrap();
    let starts: Vec<&str> = wrap.lines().collect();
    assert_eq!(starts.len(), 3, "{wrap}");
    assert!(starts[1].contains("--resume S1"), "the second start resumed: {wrap}");
    assert!(!starts[2].contains("--resume"), "the third start is a new conversation: {wrap}");
}
