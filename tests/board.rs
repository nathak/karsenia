//! The board over a real registry: the lines of plain and worktree chats, what a chat's log adds up to, the merge state worked out from
//! real git, the unsafe-settings rule, archived chats, and the `board` message of the WebSocket.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tenrec::board::{Merge, Row};
use tenrec::registry::Registry;
use tenrec::server::{start, start_core, Config};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").current_dir(dir).args(args).env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

struct Rig {
    _dir: tempfile::TempDir,
    base: PathBuf,
    project: PathBuf,
}

/// A project with one commit (a git repository), and the config of a server whose `claude` plays `scenario`.
fn rig(scenario: &str) -> (Rig, Config) {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let project = base.join("project");
    std::fs::create_dir_all(&project).unwrap();
    git(&project, &["-c", "init.defaultBranch=main", "init"]);
    git(&project, &["config", "user.name", "t"]);
    git(&project, &["config", "user.email", "t@t"]);
    std::fs::write(project.join("a.txt"), "one\n").unwrap();
    git(&project, &["add", "."]);
    git(&project, &["commit", "-m", "first"]);
    std::fs::create_dir_all(base.join("data")).unwrap();
    std::fs::write(base.join("data").join("settings.json"), json!({"worktree_root": base.join("wts")}).to_string()).unwrap();
    std::fs::write(base.join("scenario.jsonl"), scenario).unwrap();
    let wrapper = base.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' exec '{}' \"$@\"\n", base.join("scenario.jsonl").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let config = Config { port: 0, data_dir: base.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() };
    (Rig { _dir: dir, base, project }, config)
}

const WAIT: &str = "{\"$wait\":\"user\"}\n";

fn tool(id: &str, name: &str, input: &str) -> String {
    format!("{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"tool_use\",\"id\":\"{id}\",\"name\":\"{name}\",\"input\":{input}}}],\"usage\":{{\"input_tokens\":100,\"output_tokens\":5}}}}}}\n")
}

fn done(total: f64) -> String {
    format!("{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"num_turns\":1,\"total_cost_usd\":{total},\"modelUsage\":{{\"m\":{{\"contextWindow\":1000}}}}}}\n")
}

async fn idle(rx: &mut tokio::sync::broadcast::Receiver<tenrec::session::Envelope>) {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.expect("timed out").unwrap();
        if got.kind == "state" && got.data["state"] == "idle" {
            return;
        }
    }
}

/// The board once every worktree has been looked at (or has had its look end in something).
async fn settled(registry: &Arc<Registry>) -> Vec<Row> {
    for _ in 0..300 {
        let rows = registry.board(false).await;
        if rows.iter().all(|r| r.wt.as_ref().is_none_or(|w| w.merge != Merge::Unknown && !w.refreshing)) {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the worktrees were never looked at");
}

fn by_sid<'a>(rows: &'a [Row], sid: &str) -> &'a Row {
    rows.iter().find(|r| r.sid == sid).unwrap_or_else(|| panic!("no row for {sid}"))
}

#[tokio::test]
async fn a_plain_chat_has_a_line_with_its_tools_cost_and_no_worktree_data_and_the_line_follows_the_chat() {
    let scenario = format!("{WAIT}{}{}{}{WAIT}{}{}{}", tool("t1", "Edit", "{\"file_path\":\"x.rs\"}"), tool("t2", "Edit", "{\"file_path\":\"y.rs\"}"), done(0.5), tool("t3", "Bash", "{\"command\":\"ls\"}"), done(0.75), WAIT);
    let (r, config) = rig(&scenario);
    let registry = start_core(config).unwrap();
    let chat = registry.create_in(r.project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("first".into(), vec![]).await.unwrap();
    idle(&mut rx).await;

    // The first ask folds the log that is already there.
    let rows = registry.board(false).await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!((row.state, row.pending, row.worktree, row.wt.is_none()), ("idle", 0, false, true));
    assert_eq!((row.counters.tools, row.counters.edited, row.counters.turns), (2, 2, 1));
    assert_eq!(row.counters.top_tool, Some(("Edit".to_owned(), 2)));
    assert!((row.cost - 0.5).abs() < 1e-9, "{}", row.cost);
    assert_eq!((row.ctx_pct, row.counters.last_ok), (Some(10), Some(true)));
    assert!(row.counters.last_ts > 0 && row.signals.is_empty(), "{:?}", row.signals);

    // After that the chat feeds its digest itself.
    chat.send("second".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    let row = registry.board(false).await.remove(0);
    assert_eq!((row.counters.tools, row.counters.turns), (3, 2));
    assert_eq!(row.counters.top_tool, Some(("Bash".to_owned(), 1)), "the latest turn's own count");
    assert!((row.cost - 0.75).abs() < 1e-9, "{}", row.cost);
}

#[tokio::test]
async fn an_archived_idle_chat_is_not_on_the_board() {
    let (r, config) = rig(&format!("{WAIT}{}{WAIT}", done(0.1)));
    let registry = start_core(config).unwrap();
    let chat = registry.create_in(r.project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("hi".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    assert_eq!(registry.board(false).await.len(), 1);
    chat.set_archived(true).await.unwrap();
    assert!(registry.board(false).await.is_empty());
}

fn commit_in(dir: &Path, file: &str, text: &str, message: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", message]);
}

#[tokio::test]
async fn the_merge_column_is_worked_out_from_real_git_for_every_worktree_chat_of_a_project() {
    let (r, config) = rig(WAIT);
    let registry = start_core(config).unwrap();
    let cwd = r.project.to_str().unwrap();
    let mut chats = Vec::new();
    for _ in 0..5 {
        let (chat, _) = registry.create_isolated(cwd, None, None, None).await.unwrap();
        chats.push((chat.sid.clone(), chat.info().await.unwrap().meta.cwd));
    }
    let [nothing, ready, uncommitted, conflict, behind] = &chats[..] else { unreachable!() };
    // ready: one commit of its own. uncommitted: a commit and a loose file. conflict: edits a.txt as the main folder will.
    commit_in(&ready.1, "b.txt", "b\n", "b");
    commit_in(&uncommitted.1, "c.txt", "c\n", "c");
    std::fs::write(uncommitted.1.join("loose.txt"), "x\n").unwrap();
    commit_in(&conflict.1, "a.txt", "theirs\n", "theirs");
    commit_in(&behind.1, "d.txt", "d\n", "d");
    commit_in(&r.project, "a.txt", "ours\n", "ours");

    let rows = settled(&registry).await;
    assert_eq!(rows.len(), 5);
    let merge = |sid: &str| by_sid(&rows, sid).wt.clone().unwrap().merge;
    assert_eq!(merge(&nothing.0), Merge::Nothing);
    // The main folder moved on by one commit for every chat: those with a clean branch are ready though behind.
    assert_eq!(merge(&ready.0), Merge::Behind { n: 1 });
    assert_eq!(merge(&uncommitted.0), Merge::Uncommitted { n: 1 });
    assert_eq!(merge(&conflict.0), Merge::Conflict { n: 1 });
    assert_eq!(merge(&behind.0), Merge::Behind { n: 1 });
    let wt = by_sid(&rows, &ready.0).wt.clone().unwrap();
    assert_eq!((wt.ahead, wt.behind, wt.files, wt.insertions, wt.deletions), (1, 1, 1, 1, 0));
    assert!(rows.iter().all(|r| r.worktree && r.wt.as_ref().is_some_and(|w| !w.refreshing)));
}

#[tokio::test]
async fn settings_that_can_run_a_program_make_the_line_unsafe_and_nothing_runs_until_the_person_accepts_them_in_the_chat() {
    let (r, config) = rig(WAIT);
    let registry = start_core(config).unwrap();
    let (chat, _) = registry.create_isolated(r.project.to_str().unwrap(), None, None, None).await.unwrap();
    let mark = r.base.join("ran");
    git(&r.project, &["config", "filter.x.clean", &format!("touch {}", mark.display())]);
    let rows = settled(&registry).await;
    assert_eq!(by_sid(&rows, &chat.sid).wt.clone().unwrap().merge, Merge::UnsafeConfig);
    assert!(!mark.exists(), "looking at the board ran nothing");
}

#[tokio::test]
async fn a_worktree_deleted_by_hand_is_gone() {
    let (r, config) = rig(WAIT);
    let registry = start_core(config).unwrap();
    let (chat, _) = registry.create_isolated(r.project.to_str().unwrap(), None, None, None).await.unwrap();
    // The folder is deleted by hand and git forgets it: the chat's worktree is gone.
    let folder = chat.info().await.unwrap().meta.cwd;
    git(&r.project, &["worktree", "remove", "--force", folder.to_str().unwrap()]);
    git(&r.project, &["worktree", "prune"]);
    let rows = settled(&registry).await;
    assert_eq!(by_sid(&rows, &chat.sid).wt.clone().unwrap().merge, Merge::Gone);
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(port: u16, token: &str) -> Ws {
    let mut request = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
    request.headers_mut().insert("Origin", HeaderValue::from_str(&format!("http://127.0.0.1:{port}")).unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    ws.send(Message::Text(json!({"op": "auth", "token": token}).to_string())).await.unwrap();
    ws
}

#[tokio::test]
async fn the_websocket_board_message_carries_the_same_rows() {
    let (r, config) = rig(&format!("{WAIT}{}{WAIT}", done(0.2)));
    let server = start(config).await.unwrap();
    let chat = server.registry.create_in(r.project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send("hi".into(), vec![]).await.unwrap();
    idle(&mut rx).await;
    let mut ws = connect(server.addr.port(), &server.token).await;
    ws.send(Message::Text(json!({"op": "board"}).to_string())).await.unwrap();
    let reply = loop {
        let got = tokio::time::timeout(Duration::from_secs(10), ws.next()).await.expect("timed out").expect("closed").expect("error");
        if let Message::Text(text) = got {
            let v: Value = serde_json::from_str(&text).unwrap();
            if v["kind"] == "board" {
                break v;
            }
        }
    };
    let rows = reply["data"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0]["sid"].as_str().unwrap(), rows[0]["state"].as_str().unwrap(), rows[0]["worktree"].as_bool().unwrap()), (chat.sid.as_str(), "idle", false));
    assert!(rows[0]["wt"].is_null() && rows[0]["cost"].as_f64().unwrap() > 0.0);
}
