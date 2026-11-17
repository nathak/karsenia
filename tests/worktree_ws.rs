//! A chat in its own git worktree over the WebSocket: made from the project's commit on a new branch, checked, merged exactly as
//! shown, removed with care, and refused for a worktree this app did not make. Uses real git in temporary folders.
#![allow(clippy::result_large_err)]
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tenrec::server::{start, Config, Running};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").current_dir(dir).args(args).env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

struct Fx {
    dir: tempfile::TempDir,
    server: Running,
    project: PathBuf,
    root: PathBuf,
}

fn config(root: &Path, scenario: &str) -> Config {
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data").join("settings.json"), json!({"worktree_root": root.join("wts")}).to_string()).unwrap();
    let scenario_file = root.join("scenario.jsonl");
    std::fs::write(&scenario_file, scenario).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' FAKE_ENV_LOG='{}' exec '{}' \"$@\"\n", scenario_file.display(), root.join("fake.log").display(), root.join("fake.env").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }
}

fn config_keeping_settings(root: &Path) -> Config {
    let saved = std::fs::read(root.join("data").join("settings.json")).unwrap();
    let config = config(root, "{\"$wait\":\"user\"}\n");
    std::fs::write(root.join("data").join("settings.json"), saved).unwrap();
    config
}

async fn fixture_with(scenario: &str) -> Fx {
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
    let server = start(config(&base, scenario)).await.unwrap();
    Fx { root: base.join("wts"), dir, server, project }
}

async fn fixture() -> Fx {
    fixture_with("{\"$wait\":\"user\"}\n").await
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(server: &Running) -> Ws {
    let port = server.addr.port();
    let mut request = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
    request.headers_mut().insert("Origin", HeaderValue::from_str(&format!("http://127.0.0.1:{port}")).unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    ws.send(Message::Text(json!({"op": "auth", "token": server.token}).to_string())).await.unwrap();
    ws
}

async fn good(fx: &Fx) -> Ws {
    connect(&fx.server).await
}

async fn send(ws: &mut Ws, op: Value) {
    ws.send(Message::Text(op.to_string())).await.unwrap();
}

/// The next message whose kind is one of `kinds`.
async fn until(ws: &mut Ws, kinds: &[&str]) -> Value {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(20), ws.next()).await.expect("timed out").expect("closed").expect("error");
        if let Message::Text(text) = got {
            let v: Value = serde_json::from_str(&text).unwrap();
            if kinds.iter().any(|k| v["kind"] == *k) {
                return v;
            }
        }
    }
}

/// Asks for a chat in a worktree: `created` on success, `error` otherwise.
async fn new_isolated(ws: &mut Ws, cwd: &Path) -> Value {
    send(ws, json!({"op": "new", "cwd": cwd.display().to_string(), "worktree": true})).await;
    until(ws, &["created", "error"]).await
}

async fn ask(ws: &mut Ws, op: Value) -> Value {
    send(ws, op).await;
    until(ws, &["wt_status", "wt_merged", "wt_removed", "wt_trusted", "wt_error"]).await
}

fn commit_in(dir: &Path, file: &str, text: &str, message: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", message]);
}

#[tokio::test]
async fn a_chat_starts_in_a_new_worktree_on_its_own_branch_and_claude_runs_there() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let created = new_isolated(&mut ws, &fx.project).await;
    assert_eq!(created["kind"], "created", "{created}");
    let sid = created["sid"].as_str().unwrap().to_owned();
    let data = &created["data"];
    let cwd = PathBuf::from(data["cwd"].as_str().unwrap());
    assert!(cwd.starts_with(&fx.root) && cwd.ends_with(&sid), "{cwd:?}");
    assert_eq!(data["worktree"]["branch"], format!("tenrec/{sid}"));
    assert_eq!(data["worktree"]["base_branch"], "main");
    assert_eq!(data["worktree"]["repo"], fx.project.display().to_string());
    assert!(data["notice"].is_null());
    assert!(git(&fx.project, &["worktree", "list"]).contains(&cwd.display().to_string()));
    assert_eq!(git(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"]), format!("tenrec/{sid}"));
    // The project itself is untouched: still on main, clean.
    assert_eq!(git(&fx.project, &["status", "--porcelain"]), "");
    send(&mut ws, json!({"op": "send", "sid": sid, "text": "hi"})).await;
    for _ in 0..100 {
        if std::fs::read_to_string(fx.dir.path().join("fake.env")).map(|t| t.contains(&cwd.display().to_string())).unwrap_or(false) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("claude did not start in the worktree: {:?}", std::fs::read_to_string(fx.dir.path().join("fake.env")));
}

#[tokio::test]
async fn what_cannot_be_isolated_is_refused_with_a_reason() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let plain = fx.dir.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let e = new_isolated(&mut ws, &plain).await;
    assert!(e["data"]["message"].as_str().unwrap().contains("not a git project"), "{e}");
    // Together with a container: refused before anything is made.
    send(&mut ws, json!({"op": "new", "cwd": fx.project.display().to_string(), "worktree": true, "container": true})).await;
    let e = until(&mut ws, &["created", "error"]).await;
    assert!(e["data"]["message"].as_str().unwrap().contains("container"), "{e}");
    assert!(!fx.root.exists() || std::fs::read_dir(&fx.root).unwrap().next().is_none(), "nothing was made");
    // A worktree is not a project to branch from.
    let first = new_isolated(&mut ws, &fx.project).await;
    let wt = first["data"]["cwd"].as_str().unwrap().to_owned();
    let e = new_isolated(&mut ws, Path::new(&wt)).await;
    assert!(e["data"]["message"].as_str().unwrap().contains("already a git worktree"), "{e}");
    // A folder inside the project starts at the worktree's top, with a note.
    let sub = fx.project.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let from_sub = new_isolated(&mut ws, &sub).await;
    assert_eq!(from_sub["kind"], "created", "{from_sub}");
    assert_eq!(from_sub["data"]["notice"], "below_top");
    // A detached HEAD has no branch to merge back to.
    git(&fx.project, &["checkout", "--detach"]);
    let e = new_isolated(&mut ws, &fx.project).await;
    assert!(e["data"]["message"].as_str().unwrap().contains("detached"), "{e}");
}

#[tokio::test]
async fn the_check_the_merge_and_the_removal_follow_what_was_shown() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let created = new_isolated(&mut ws, &fx.project).await;
    let sid = created["sid"].as_str().unwrap().to_owned();
    let cwd = PathBuf::from(created["data"]["cwd"].as_str().unwrap());
    let status = ask(&mut ws, json!({"op": "wt_status", "sid": sid, "req": 1})).await;
    assert_eq!(status["data"]["blockers"], json!(["nothing"]));
    assert_eq!(status["data"]["req"], 1);
    commit_in(&cwd, "b.txt", "two\n", "second");
    std::fs::write(cwd.join("loose.txt"), "x").unwrap();
    let status = ask(&mut ws, json!({"op": "wt_status", "sid": sid, "req": 2})).await;
    assert_eq!(status["data"]["blockers"], json!(["uncommitted"]));
    // Removing with changes needs force.
    let e = ask(&mut ws, json!({"op": "wt_remove", "sid": sid, "req": 3})).await;
    assert_eq!(e["data"]["code"], "uncommitted", "{e}");
    std::fs::remove_file(cwd.join("loose.txt")).unwrap();
    let status = ask(&mut ws, json!({"op": "wt_status", "sid": sid, "req": 4})).await;
    let (tip, head) = (status["data"]["tip"].as_str().unwrap().to_owned(), status["data"]["main_head"].as_str().unwrap().to_owned());
    assert_eq!(status["data"]["blockers"], json!([]));
    assert_eq!((status["data"]["ahead"].as_u64(), status["data"]["files"].as_u64(), status["data"]["insertions"].as_u64()), (Some(1), Some(1), Some(1)));
    // Someone commits in the project after the check: what was shown is no longer what would merge.
    commit_in(&fx.project, "c.txt", "three\n", "meanwhile");
    let e = ask(&mut ws, json!({"op": "wt_merge", "sid": sid, "req": 5, "tip": tip, "main_head": head})).await;
    assert_eq!(e["data"]["code"], "stale", "{e}");
    let status = ask(&mut ws, json!({"op": "wt_status", "sid": sid, "req": 6})).await;
    let (tip, head) = (status["data"]["tip"].as_str().unwrap().to_owned(), status["data"]["main_head"].as_str().unwrap().to_owned());
    let merged = ask(&mut ws, json!({"op": "wt_merge", "sid": sid, "req": 7, "tip": tip, "main_head": head})).await;
    assert_eq!(merged["kind"], "wt_merged", "{merged}");
    assert_eq!(git(&fx.project, &["rev-parse", "HEAD"]), merged["data"]["commit"].as_str().unwrap());
    assert_eq!(git(&fx.project, &["rev-parse", "HEAD^2"]), tip, "the second parent is the commit that was shown");
    assert!(fx.project.join("b.txt").exists() && fx.project.join("c.txt").exists());
    let removed = ask(&mut ws, json!({"op": "wt_remove", "sid": sid, "req": 8})).await;
    assert_eq!(removed["kind"], "wt_removed", "{removed}");
    assert_eq!(removed["data"]["branch_kept"], false, "it was merged, so the branch goes too");
    assert!(!cwd.exists());
    assert!(!git(&fx.project, &["branch", "--list", &format!("tenrec/{sid}")]).contains("tenrec"));
    // The chat can no longer be written to, and no further worktree operation applies.
    send(&mut ws, json!({"op": "send", "sid": sid, "text": "more"})).await;
    let e = until(&mut ws, &["error"]).await;
    assert!(e["data"]["message"].as_str().unwrap().contains("removed"), "{e}");
    let e = ask(&mut ws, json!({"op": "wt_status", "sid": sid, "req": 9})).await;
    assert_eq!(e["data"]["code"], "removed");
    // It is remembered after a restart: the list shows it as removed.
    send(&mut ws, json!({"op": "list"})).await;
    let hello = until(&mut ws, &["hello"]).await;
    let found = hello["data"]["sessions"].as_array().unwrap().iter().find(|s| s["sid"] == sid.as_str()).unwrap().clone();
    assert_eq!(found["worktree"]["removed"], true);
}

#[tokio::test]
async fn a_conflict_changes_nothing_and_unmerged_work_keeps_its_branch() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let created = new_isolated(&mut ws, &fx.project).await;
    let sid = created["sid"].as_str().unwrap().to_owned();
    let cwd = PathBuf::from(created["data"]["cwd"].as_str().unwrap());
    commit_in(&cwd, "a.txt", "mine\n", "mine");
    commit_in(&fx.project, "a.txt", "theirs\n", "theirs");
    let head = git(&fx.project, &["rev-parse", "HEAD"]);
    let status = ask(&mut ws, json!({"op": "wt_status", "sid": sid, "req": 1})).await;
    assert_eq!(status["data"]["blockers"], json!(["conflict"]));
    assert_eq!(status["data"]["conflicts"], json!(["a.txt"]));
    let (tip, main_head) = (status["data"]["tip"].as_str().unwrap().to_owned(), status["data"]["main_head"].as_str().unwrap().to_owned());
    let e = ask(&mut ws, json!({"op": "wt_merge", "sid": sid, "req": 2, "tip": tip, "main_head": main_head})).await;
    assert_eq!(e["data"]["code"], "blocked");
    assert_eq!(git(&fx.project, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&fx.project, &["status", "--porcelain"]), "");
    // Removing unmerged work: the folder goes, the branch stays (it holds the only copy of the commit).
    let removed = ask(&mut ws, json!({"op": "wt_remove", "sid": sid, "req": 3})).await;
    assert_eq!(removed["data"]["branch_kept"], true, "{removed}");
    assert!(git(&fx.project, &["branch", "--list", &format!("tenrec/{sid}")]).contains("tenrec"));
    assert!(!cwd.exists());
}

#[tokio::test]
async fn a_turn_in_progress_stops_the_removal() {
    let fx = fixture_with("{\"$wait\":\"user\"}\n{\"$wait\":\"interrupt\"}\n").await;
    let mut ws = good(&fx).await;
    let created = new_isolated(&mut ws, &fx.project).await;
    let sid = created["sid"].as_str().unwrap().to_owned();
    let cwd = PathBuf::from(created["data"]["cwd"].as_str().unwrap());
    send(&mut ws, json!({"op": "send", "sid": sid, "text": "work"})).await;
    let e = ask(&mut ws, json!({"op": "wt_remove", "sid": sid, "req": 1, "force": true})).await;
    assert_eq!(e["data"]["code"], "busy", "expected busy while the turn runs: {e}");
    assert!(cwd.exists(), "{e}");
}

#[tokio::test]
async fn a_worktree_folder_deleted_by_hand_is_noticed_and_can_be_cleared() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let created = new_isolated(&mut ws, &fx.project).await;
    let sid = created["sid"].as_str().unwrap().to_owned();
    let cwd = PathBuf::from(created["data"]["cwd"].as_str().unwrap());
    std::fs::remove_dir_all(&cwd).unwrap();
    let status = ask(&mut ws, json!({"op": "wt_status", "sid": sid, "req": 1})).await;
    assert_eq!(status["data"]["missing"], true);
    send(&mut ws, json!({"op": "send", "sid": sid, "text": "hi"})).await;
    let e = until(&mut ws, &["error"]).await;
    assert!(e["data"]["message"].as_str().unwrap().contains("gone"), "{e}");
    let removed = ask(&mut ws, json!({"op": "wt_remove", "sid": sid, "req": 2})).await;
    assert_eq!(removed["kind"], "wt_removed", "{removed}");
    assert_eq!(git(&fx.project, &["worktree", "list"]).lines().count(), 1);
}

/// What a changed meta file could aim at: nothing outside the worktree root is touched, whatever it says.
#[tokio::test]
async fn a_meta_file_that_points_elsewhere_is_refused_and_nothing_is_touched() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let created = new_isolated(&mut ws, &fx.project).await;
    let sid = created["sid"].as_str().unwrap().to_owned();
    let victim = fx.dir.path().canonicalize().unwrap().join("victim");
    std::fs::create_dir_all(&victim).unwrap();
    std::fs::write(victim.join("keep.txt"), "mine").unwrap();
    git(&victim, &["-c", "init.defaultBranch=main", "init"]);
    git(&victim, &["config", "user.name", "t"]);
    git(&victim, &["config", "user.email", "t@t"]);
    git(&victim, &["add", "."]);
    git(&victim, &["commit", "-m", "x"]);
    fx.server.task.abort();
    let meta_file = fx.dir.path().canonicalize().unwrap().join("data").join(format!("{sid}.meta.json"));
    let original: Value = serde_json::from_str(&std::fs::read_to_string(&meta_file).unwrap()).unwrap();
    type Edit<'a> = Box<dyn Fn(&mut Value) + 'a>;
    let edits: Vec<(&str, Edit)> = vec![
        ("cwd outside", Box::new(|m| m["cwd"] = json!(victim.display().to_string()))),
        ("branch is main", Box::new(|m| m["worktree"]["branch"] = json!("main"))),
        ("another repo", Box::new(|m| m["worktree"]["repo"] = json!(victim.display().to_string()))),
    ];
    for (what, edit) in edits {
        let mut meta = original.clone();
        edit(&mut meta);
        std::fs::write(&meta_file, meta.to_string()).unwrap();
        let server = start(config(&fx.dir.path().canonicalize().unwrap(), "{\"$wait\":\"user\"}\n")).await.unwrap();
        let mut ws = connect(&server).await;
        for op in ["wt_status", "wt_merge", "wt_remove"] {
            let r = ask(&mut ws, json!({"op": op, "sid": sid, "req": 1, "tip": "x", "main_head": "y", "force": true, "discard": true})).await;
            assert_eq!(r["data"]["code"], "not_owned", "{what} / {op}: {r}");
        }
        server.task.abort();
    }
    assert_eq!(std::fs::read_to_string(victim.join("keep.txt")).unwrap(), "mine");
    assert_eq!(git(&victim, &["branch", "--list"]).trim_start_matches("* "), "main");
    assert_eq!(git(&fx.project, &["branch", "--list", "main"]).trim_start_matches("* "), "main");
}

#[tokio::test]
async fn an_old_meta_file_without_a_worktree_is_still_a_plain_chat() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    send(&mut ws, json!({"op": "new", "cwd": fx.project.display().to_string()})).await;
    let created = until(&mut ws, &["created", "error"]).await;
    assert!(created["data"]["worktree"].is_null());
    let sid = created["sid"].as_str().unwrap().to_owned();
    let r = ask(&mut ws, json!({"op": "wt_status", "sid": sid, "req": 1})).await;
    assert_eq!(r["data"]["code"], "no_worktree");
}

#[tokio::test]
async fn settings_that_can_run_a_program_must_be_accepted_by_a_person_before_anything_runs() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let created = new_isolated(&mut ws, &fx.project).await;
    let sid = created["sid"].as_str().unwrap().to_owned();
    let mark = fx.dir.path().join("ran");
    // What a command run in the chat could do: put a filter in the shared settings.
    git(&fx.project, &["config", "filter.x.clean", &format!("touch {}", mark.display())]);
    let e = ask(&mut ws, json!({"op": "wt_status", "sid": sid, "req": 1})).await;
    assert_eq!(e["data"]["code"], "config_changed", "{e}");
    assert!(!mark.exists(), "looking ran nothing");
    assert!(e["data"]["entries"].as_array().unwrap().iter().any(|x| x.as_str().unwrap().starts_with("filter.x.clean=")));
    let hash = e["data"]["hash"].as_str().unwrap().to_owned();
    for op in ["wt_merge", "wt_remove"] {
        let r = ask(&mut ws, json!({"op": op, "sid": sid, "req": 2, "tip": "x", "main_head": "y"})).await;
        assert_eq!(r["data"]["code"], "config_changed", "{op}: {r}");
    }
    // Only what was shown is accepted.
    let wrong = ask(&mut ws, json!({"op": "wt_trust", "sid": sid, "req": 3, "hash": "0000000000000000"})).await;
    assert_eq!(wrong["data"]["code"], "config_changed");
    let trusted = ask(&mut ws, json!({"op": "wt_trust", "sid": sid, "req": 4, "hash": hash})).await;
    assert_eq!(trusted["kind"], "wt_trusted", "{trusted}");
    let status = ask(&mut ws, json!({"op": "wt_status", "sid": sid, "req": 5})).await;
    assert_eq!(status["kind"], "wt_status", "{status}");
}

#[tokio::test]
async fn a_changed_worktree_folder_setting_does_not_strand_the_chats_made_before() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let created = new_isolated(&mut ws, &fx.project).await;
    let sid = created["sid"].as_str().unwrap().to_owned();
    fx.server.task.abort();
    let base = fx.dir.path().canonicalize().unwrap();
    std::fs::write(base.join("data").join("settings.json"), json!({"worktree_root": base.join("elsewhere")}).to_string()).unwrap();
    let server = start(config_keeping_settings(&base)).await.unwrap();
    let mut ws = connect(&server).await;
    let status = ask(&mut ws, json!({"op": "wt_status", "sid": sid, "req": 1})).await;
    assert_eq!(status["kind"], "wt_status", "{status}");
    // A new chat goes under the new folder.
    let again = new_isolated(&mut ws, &fx.project).await;
    assert!(PathBuf::from(again["data"]["cwd"].as_str().unwrap()).starts_with(base.join("elsewhere")), "{again}");
}

#[tokio::test]
async fn a_chat_whose_worktree_is_gone_from_disk_and_from_git_can_still_be_cleared() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let created = new_isolated(&mut ws, &fx.project).await;
    let sid = created["sid"].as_str().unwrap().to_owned();
    let cwd = PathBuf::from(created["data"]["cwd"].as_str().unwrap());
    std::fs::remove_dir_all(&cwd).unwrap();
    git(&fx.project, &["worktree", "prune"]);
    let removed = ask(&mut ws, json!({"op": "wt_remove", "sid": sid, "req": 1})).await;
    assert_eq!(removed["kind"], "wt_removed", "{removed}");
    assert_eq!(removed["data"]["warning"], "forgotten");
    send(&mut ws, json!({"op": "send", "sid": sid, "text": "x"})).await;
    let e = until(&mut ws, &["error"]).await;
    assert!(e["data"]["message"].as_str().unwrap().contains("removed"));
}
