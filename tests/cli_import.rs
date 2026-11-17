//! A session of the `claude` command line program brought into a chat: its conversation is in the chat's log from the start, the
//! chat remembers the CLI's session id, nothing is started until the first message, and that message resumes the session.
use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use tenrec::claude::protocol::Image;
use tenrec::cli_sessions::{encode_dir, list, read_history};
use tenrec::registry::Registry;
use tenrec::session::Envelope;
use tokio::sync::broadcast::Receiver;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");
const CLI_ID: &str = "11111111-2222-3333-4444-555555555555";

struct Fx {
    _dir: tempfile::TempDir,
    root: PathBuf,
    registry: Registry,
    log: PathBuf,
    project: PathBuf,
    config: PathBuf,
}

fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let scenario = root.join("scenario.jsonl");
    std::fs::write(&scenario, "{\"$wait\":\"user\"}\n{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"NEW\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n{\"$exit\":0}\n").unwrap();
    let log = root.join("fake.log");
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", scenario.display(), log.display(), FAKE)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    // The CLI's own folder for this project, with one session in it.
    let config = root.join("cli-config");
    let cwd = project.to_str().unwrap();
    let folder = config.join("projects").join(encode_dir(cwd));
    std::fs::create_dir_all(&folder).unwrap();
    let line = |kind: &str, uuid: &str, parent: Option<&str>, message: serde_json::Value, ts: &str| json!({"type": kind, "uuid": uuid, "parentUuid": parent, "cwd": cwd, "sessionId": CLI_ID, "gitBranch": "main", "timestamp": ts, "message": message}).to_string();
    let lines = [
        line("user", "u1", None, json!({"role": "user", "content": "what is in main.rs?"}), "2026-11-13T05:00:00.000Z"),
        line("assistant", "a1", Some("u1"), json!({"role": "assistant", "content": [{"type": "text", "text": "a small program"}, {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}}]}), "2026-11-13T05:00:01.000Z"),
    ];
    std::fs::write(folder.join(format!("{CLI_ID}.jsonl")), lines.join("\n") + "\n").unwrap();
    let registry = Registry::open(root.join("data"), Some(wrapper.display().to_string())).unwrap();
    Fx { _dir: dir, root, registry, log, project, config }
}

async fn until(rx: &mut Receiver<Envelope>, what: impl Fn(&Envelope) -> bool) -> Envelope {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.expect("timed out").expect("the stream ended");
        if what(&got) {
            return got;
        }
    }
}

#[tokio::test]
async fn an_earlier_session_comes_in_with_its_conversation_and_the_first_message_resumes_it() {
    let fx = fixture();
    let found = list(&fx.config, 10);
    assert_eq!(found.len(), 1);
    let session = &found[0];
    assert!(session.folder_ok && session.title == "what is in main.rs?", "{session:?}");
    let history = read_history(&session.path, 400, 4 << 20).unwrap();
    let handle = fx.registry.import_cli(&session.cwd, &session.id, &session.title, None, history).await.unwrap();

    // The chat remembers the CLI's session id and was given the conversation; claude was not started.
    let info = handle.info().await.unwrap();
    assert_eq!(info.meta.claude_session_id.as_deref(), Some(CLI_ID));
    assert_eq!(info.meta.imported_from.as_deref(), Some(CLI_ID));
    assert_eq!(info.meta.title, "what is in main.rs?");
    assert!(!fx.log.exists(), "nothing is started at import");
    let (events, mut rx) = handle.subscribe(0).await.unwrap();
    let kinds: Vec<(u64, &str)> = events.iter().map(|e| (e.seq, e.kind.as_str())).collect();
    assert_eq!(kinds, vec![(1, "imported"), (2, "user"), (3, "claude")]);
    assert_eq!(events[1].data["text"], "what is in main.rs?");

    // What was imported is not this chat's own writing: auto-save does not read it.
    assert!(handle.user_texts(0, 0).await.unwrap().is_empty());
    // Nor does a skill proposal: its messages, tool calls and results are not this chat's own work.
    let input = handle.proposal_input(0).await.unwrap();
    assert!(input.user_texts.is_empty() && input.tool_lines.is_empty() && input.last_turn_tools == 0, "{:?}", (input.user_texts, input.tool_lines));

    // The same session cannot be imported twice.
    let again = read_history(&session.path, 400, 4 << 20).unwrap();
    assert!(fx.registry.import_cli(&session.cwd, &session.id, "x", None, again).await.err().expect("refused").contains("already imported"));

    // The first message resumes it, from the folder the CLI kept it in.
    handle.send("and the tests?".into(), Vec::<Image>::new()).await.unwrap();
    until(&mut rx, |e| e.kind == "state" && e.data["state"] == "exited").await;
    let log = std::fs::read_to_string(&fx.log).unwrap();
    let argv = log.lines().find(|l| l.starts_with("ARGV")).expect("started");
    assert!(argv.contains(&format!("--resume {CLI_ID}")), "{argv}");

    // The chat is the same after the app starts again.
    drop(handle);
    let reopened = Registry::open(fx.root.join("data"), None).unwrap();
    let list = reopened.list().await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].meta.imported_from.as_deref(), Some(CLI_ID));
    let (again, _) = reopened.get(&list[0].meta.sid).unwrap().subscribe(0).await.unwrap();
    assert!(again.len() >= 3 && again.iter().map(|e| e.seq).eq(1..=again.len() as u64), "the log is whole and numbered in order");
}

#[tokio::test]
async fn a_bad_id_or_a_folder_the_cli_did_not_keep_it_in_is_refused() {
    let fx = fixture();
    let empty = || read_history(&list(&fx.config, 1)[0].path, 400, 4 << 20).unwrap();
    assert!(fx.registry.import_cli(fx.project.to_str().unwrap(), "../../etc/passwd", "t", None, empty()).await.is_err());
    assert!(fx.registry.import_cli("relative/path", CLI_ID, "t", None, empty()).await.is_err());
    // A folder that makes another name (a link to the project) is not where the CLI kept the session.
    #[cfg(unix)]
    {
        let link = fx.root.join("linked");
        std::os::unix::fs::symlink(&fx.project, &link).unwrap();
        let err = fx.registry.import_cli(link.to_str().unwrap(), CLI_ID, "t", None, empty()).await.err().expect("refused");
        assert!(err.contains("not where the session was kept"), "{err}");
    }
    assert!(fx.registry.list().await.is_empty(), "a refusal leaves no chat behind");
}
