//! The file tree and editor over the WebSocket: list, read, save, a conflict, a confirmation, and what must not work.
#![allow(clippy::result_large_err)]
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tenrec::server::{start, Config, Running};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

struct Fx {
    _dir: tempfile::TempDir,
    server: Running,
    project: std::path::PathBuf,
}

async fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let scenario = root.join("scenario.jsonl");
    std::fs::write(&scenario, "{\"$wait\":\"user\"}\n").unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", scenario.display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::create_dir_all(project.join(".claude")).unwrap();
    std::fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(project.join(".claude/settings.json"), "{}").unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    Fx { _dir: dir, server, project }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn good(fx: &Fx) -> Ws {
    let port = fx.server.addr.port();
    let mut request = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
    request.headers_mut().insert("Origin", HeaderValue::from_str(&format!("http://127.0.0.1:{port}")).unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    ws.send(Message::Text(json!({"op": "auth", "token": fx.server.token}).to_string())).await.unwrap();
    ws
}

async fn next_json(ws: &mut Ws) -> Value {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), ws.next()).await.expect("timed out").expect("closed").expect("error");
        if let Message::Text(text) = got {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

async fn until_kind(ws: &mut Ws, kind: &str) -> Value {
    loop {
        let got = next_json(ws).await;
        if got["kind"] == kind {
            return got;
        }
    }
}

async fn open_chat(fx: &Fx, ws: &mut Ws) -> String {
    ws.send(Message::Text(json!({"op": "new", "cwd": fx.project.display().to_string()}).to_string())).await.unwrap();
    until_kind(ws, "created").await["sid"].as_str().unwrap().to_owned()
}

/// Sends one `fs_*` op and returns the first answer that carries its `req`.
async fn ask(ws: &mut Ws, op: Value) -> Value {
    let req = op["req"].clone();
    ws.send(Message::Text(op.to_string())).await.unwrap();
    loop {
        let got = next_json(ws).await;
        if got["data"]["req"] == req {
            return got;
        }
    }
}

#[tokio::test]
async fn a_chats_project_can_be_listed_read_and_saved() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let sid = open_chat(&fx, &mut ws).await;

    let list = ask(&mut ws, json!({"op": "fs_list", "sid": sid, "req": 1, "path": ""})).await;
    assert_eq!(list["kind"], "fs_listing");
    let names: Vec<&str> = list["data"]["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, [".claude", "src"]);

    let read = ask(&mut ws, json!({"op": "fs_read", "sid": sid, "req": 2, "path": "src/main.rs"})).await;
    assert_eq!(read["kind"], "fs_text");
    assert_eq!(read["data"]["text"], "fn main() {}\n");
    let version = read["data"]["version"].as_str().unwrap().to_owned();

    let saved = ask(&mut ws, json!({"op": "fs_write", "sid": sid, "req": 3, "path": "src/main.rs", "text": "fn main() { }\n", "base": version})).await;
    assert_eq!(saved["kind"], "fs_saved");
    assert_eq!(std::fs::read_to_string(fx.project.join("src/main.rs")).unwrap(), "fn main() { }\n");

    // The check answers with the version only, and with the same request number and path.
    let checked = ask(&mut ws, json!({"op": "fs_check", "sid": sid, "req": 31, "path": "src/main.rs"})).await;
    assert_eq!(checked["kind"], "fs_checked");
    assert_eq!(checked["data"]["version"], saved["data"]["version"]);
    assert_eq!((checked["data"]["req"].as_u64(), checked["data"]["path"].as_str()), (Some(31), Some("src/main.rs")));
    assert!(checked["data"].get("text").is_none());
    let missing = ask(&mut ws, json!({"op": "fs_check", "sid": sid, "req": 32, "path": "src/none.rs"})).await;
    assert_eq!((missing["kind"].as_str(), missing["data"]["req"].as_u64()), (Some("fs_error"), Some(32)));

    // The file changed under the page: a conflict that carries the new text, and nothing is written.
    std::fs::write(fx.project.join("src/main.rs"), "changed elsewhere\n").unwrap();
    let moved = ask(&mut ws, json!({"op": "fs_check", "sid": sid, "req": 33, "path": "src/main.rs"})).await;
    assert_ne!(moved["data"]["version"], saved["data"]["version"]);
    // The check does not decode: a file that became binary still has a version (the read that follows reports it).
    std::fs::write(fx.project.join("src/main.rs"), b"\0\x01binary").unwrap();
    let binary = ask(&mut ws, json!({"op": "fs_check", "sid": sid, "req": 34, "path": "src/main.rs"})).await;
    assert_eq!(binary["kind"], "fs_checked");
    std::fs::write(fx.project.join("src/main.rs"), "changed elsewhere\n").unwrap();
    let conflict = ask(&mut ws, json!({"op": "fs_write", "sid": sid, "req": 4, "path": "src/main.rs", "text": "mine", "base": saved["data"]["version"]})).await;
    assert_eq!(conflict["kind"], "fs_conflict");
    assert_eq!(conflict["data"]["text"], "changed elsewhere\n");
    assert_eq!(std::fs::read_to_string(fx.project.join("src/main.rs")).unwrap(), "changed elsewhere\n");
}

#[tokio::test]
async fn a_place_a_later_chat_reads_asks_first_and_a_wrong_path_is_an_error() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let sid = open_chat(&fx, &mut ws).await;

    let read = ask(&mut ws, json!({"op": "fs_read", "sid": sid, "req": 1, "path": ".claude/settings.json"})).await;
    assert_eq!(read["data"]["guard_kind"], "confirm");
    let base = read["data"]["version"].clone();
    let asked = ask(&mut ws, json!({"op": "fs_write", "sid": sid, "req": 2, "path": ".claude/settings.json", "text": "{\"a\":1}", "base": base})).await;
    assert_eq!(asked["kind"], "fs_needs_confirm");
    assert_eq!(std::fs::read_to_string(fx.project.join(".claude/settings.json")).unwrap(), "{}");
    let done = ask(&mut ws, json!({"op": "fs_write", "sid": sid, "req": 3, "path": ".claude/settings.json", "text": "{\"a\":1}", "base": base, "confirm": true})).await;
    assert_eq!(done["kind"], "fs_saved");

    for (req, path) in [(10, "../project/src/main.rs"), (11, "/etc/passwd"), (12, "nope.txt"), (13, "src")] {
        let bad = ask(&mut ws, json!({"op": "fs_read", "sid": sid, "req": req, "path": path})).await;
        assert_eq!(bad["kind"], "fs_error", "{path}");
    }
    let unknown = ask(&mut ws, json!({"op": "fs_read", "sid": "nope", "req": 20, "path": "src/main.rs"})).await;
    assert_eq!(unknown["kind"], "fs_error");
}

#[tokio::test]
async fn an_answer_goes_only_to_the_page_that_asked_and_nothing_is_stored() {
    let fx = fixture().await;
    let mut first = good(&fx).await;
    let sid = open_chat(&fx, &mut first).await;
    let mut second = good(&fx).await;
    let _ = until_kind(&mut second, "hello").await;

    let read = ask(&mut first, json!({"op": "fs_read", "sid": sid, "req": 1, "path": "src/main.rs"})).await;
    assert_eq!(read["kind"], "fs_text");
    // A marker sent afterwards reaches the second page; the file text must not have come before it.
    first.send(Message::Text(json!({"op": "list"}).to_string())).await.unwrap();
    second.send(Message::Text(json!({"op": "list"}).to_string())).await.unwrap();
    loop {
        let got = next_json(&mut second).await;
        assert!(!got.to_string().contains("fn main"), "the second page heard file text: {got}");
        if got["kind"] == "hello" {
            break;
        }
    }
    // Nothing of the file is in the data folder (event log, index, meta).
    fn scan(dir: &std::path::Path) -> bool {
        std::fs::read_dir(dir).unwrap().flatten().any(|e| {
            let p = e.path();
            if p.is_dir() {
                scan(&p)
            } else {
                std::fs::read(&p).map(|b| b.windows(9).any(|w| w == b"fn main()")).unwrap_or(false)
            }
        })
    }
    assert!(!scan(&fx._dir.path().join("data")), "file text reached the data folder");
}

#[tokio::test]
async fn the_editor_needs_the_token_first() {
    let fx = fixture().await;
    let port = fx.server.addr.port();
    let mut request = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
    request.headers_mut().insert("Origin", HeaderValue::from_str(&format!("http://127.0.0.1:{port}")).unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    ws.send(Message::Text(json!({"op": "fs_list", "sid": "x", "req": 1, "path": ""}).to_string())).await.unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return true,
                Some(Ok(Message::Text(_))) => return false,
                _ => {}
            }
        }
    })
    .await;
    assert_eq!(ended, Ok(true), "an op in place of the token ends the socket without an answer");
}

#[tokio::test]
async fn the_editor_is_not_offered_for_the_apps_own_data_folder() {
    let fx = fixture().await;
    let data = fx._dir.path().join("data");
    std::fs::write(data.join("secret.txt"), "private").unwrap();
    let mut ws = good(&fx).await;
    ws.send(Message::Text(json!({"op": "new", "cwd": data.display().to_string()}).to_string())).await.unwrap();
    let created = until_kind(&mut ws, "created").await;
    let sid = created["sid"].as_str().unwrap().to_owned();
    for (req, op) in [(1, "fs_list"), (2, "fs_read")] {
        let got = ask(&mut ws, json!({"op": op, "sid": sid, "req": req, "path": if op == "fs_read" { "secret.txt" } else { "" }})).await;
        assert_eq!(got["kind"], "fs_error", "{op} in the data folder");
    }
    let saved = ask(&mut ws, json!({"op": "fs_write", "sid": sid, "req": 3, "path": "secret.txt", "text": "x", "base": ""})).await;
    assert_eq!(saved["kind"], "fs_error");
    for (req, op) in [
        (4, json!({"op": "fs_create", "sid": sid, "req": 4, "path": "new.txt", "kind": "file"})),
        (5, json!({"op": "fs_delete", "sid": sid, "req": 5, "path": "secret.txt"})),
        (6, json!({"op": "fs_rename", "sid": sid, "req": 6, "path": "secret.txt", "to": "other.txt"})),
    ] {
        let got = ask(&mut ws, op).await;
        assert_eq!((got["kind"].as_str(), got["data"]["req"].as_u64()), (Some("fs_error"), Some(req)));
    }
    assert!(data.join("secret.txt").exists() && !data.join("new.txt").exists() && !data.join("other.txt").exists());
    assert_eq!(std::fs::read_to_string(data.join("secret.txt")).unwrap(), "private");
}

#[tokio::test]
async fn a_file_can_be_made_renamed_and_deleted_and_the_guard_asks_first() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let sid = open_chat(&fx, &mut ws).await;
    let p = &fx.project;

    let made = ask(&mut ws, json!({"op": "fs_create", "sid": sid, "req": 1, "path": "src/new.rs", "kind": "file"})).await;
    assert_eq!((made["kind"].as_str(), made["data"]["req"].as_u64(), made["data"]["path"].as_str(), made["data"]["kind"].as_str()), (Some("fs_created"), Some(1), Some("src/new.rs"), Some("file")));
    assert_eq!(std::fs::read(p.join("src/new.rs")).unwrap(), b"");
    let dup = ask(&mut ws, json!({"op": "fs_create", "sid": sid, "req": 2, "path": "src/new.rs", "kind": "file"})).await;
    assert_eq!(dup["kind"], "fs_error");
    let dir = ask(&mut ws, json!({"op": "fs_create", "sid": sid, "req": 3, "path": "src/sub", "kind": "dir"})).await;
    assert_eq!(dir["kind"], "fs_created");

    let moved = ask(&mut ws, json!({"op": "fs_rename", "sid": sid, "req": 4, "path": "src/new.rs", "to": "src/renamed.rs"})).await;
    assert_eq!((moved["kind"].as_str(), moved["data"]["to"].as_str()), (Some("fs_renamed"), Some("src/renamed.rs")));
    assert!(p.join("src/renamed.rs").exists() && !p.join("src/new.rs").exists());
    // To another folder of the project, and a folder into itself is an error.
    let away = ask(&mut ws, json!({"op": "fs_rename", "sid": sid, "req": 5, "path": "src/renamed.rs", "to": "renamed.rs"})).await;
    assert_eq!((away["kind"].as_str(), away["data"]["to"].as_str()), (Some("fs_renamed"), Some("renamed.rs")));
    assert!(p.join("renamed.rs").exists() && !p.join("src/renamed.rs").exists());
    let back = ask(&mut ws, json!({"op": "fs_rename", "sid": sid, "req": 51, "path": "renamed.rs", "to": "src/renamed.rs"})).await;
    assert_eq!(back["kind"], "fs_renamed");
    let into_self = ask(&mut ws, json!({"op": "fs_rename", "sid": sid, "req": 52, "path": "src", "to": "src/sub/src"})).await;
    assert_eq!(into_self["kind"], "fs_error");
    let into_claude = ask(&mut ws, json!({"op": "fs_rename", "sid": sid, "req": 53, "path": "src/renamed.rs", "to": ".claude/renamed.rs"})).await;
    assert_eq!(into_claude["kind"], "fs_needs_confirm");
    assert!(p.join("src/renamed.rs").exists());

    // A place a later chat reads asks first, and the same request with the confirmation does it.
    let ask_first = ask(&mut ws, json!({"op": "fs_create", "sid": sid, "req": 6, "path": ".claude/new.json", "kind": "file"})).await;
    assert_eq!(ask_first["kind"], "fs_needs_confirm");
    assert!(!p.join(".claude/new.json").exists());
    let confirmed = ask(&mut ws, json!({"op": "fs_create", "sid": sid, "req": 7, "path": ".claude/new.json", "kind": "file", "confirm": true})).await;
    assert_eq!(confirmed["kind"], "fs_created");

    // A file that changed since the page read it is not deleted.
    let read = ask(&mut ws, json!({"op": "fs_read", "sid": sid, "req": 8, "path": "src/main.rs"})).await;
    let version = read["data"]["version"].as_str().unwrap().to_owned();
    std::fs::write(p.join("src/main.rs"), "changed elsewhere\n").unwrap();
    let stale = ask(&mut ws, json!({"op": "fs_delete", "sid": sid, "req": 9, "path": "src/main.rs", "base": version})).await;
    assert_eq!(stale["kind"], "fs_error");
    assert!(p.join("src/main.rs").exists());
    let gone = ask(&mut ws, json!({"op": "fs_delete", "sid": sid, "req": 10, "path": "src/renamed.rs"})).await;
    assert_eq!(gone["kind"], "fs_deleted");
    assert!(!p.join("src/renamed.rs").exists());
    // It went to the trash, which the listing shows with where the entry came from, and a restore brings it back.
    let trashed = gone["data"]["trashed"].as_str().unwrap().to_owned();
    assert!(trashed.starts_with(".tenrec-trash/"), "{trashed}");
    let listing = ask(&mut ws, json!({"op": "fs_list", "sid": sid, "req": 12, "path": ".tenrec-trash"})).await;
    let entries = listing["data"]["entries"].as_array().unwrap();
    assert!(entries.iter().any(|e| trashed.ends_with(e["name"].as_str().unwrap()) && e["from"] == "src/renamed.rs"), "{entries:?}");
    let top = ask(&mut ws, json!({"op": "fs_list", "sid": sid, "req": 13, "path": ""})).await;
    assert!(top["data"]["entries"].as_array().unwrap().iter().any(|e| e["name"] == ".tenrec-trash" && e["flags"].as_array().unwrap().iter().any(|f| f == "trash")));
    let back = ask(&mut ws, json!({"op": "fs_restore", "sid": sid, "req": 14, "path": trashed})).await;
    assert_eq!((back["kind"].as_str(), back["data"]["to"].as_str()), (Some("fs_restored"), Some("src/renamed.rs")));
    assert!(p.join("src/renamed.rs").exists());
    let twice = ask(&mut ws, json!({"op": "fs_restore", "sid": sid, "req": 15, "path": trashed})).await;
    assert_eq!(twice["kind"], "fs_error");
    // A folder with something in it can be deleted too (it goes to the trash whole).
    let dir = ask(&mut ws, json!({"op": "fs_delete", "sid": sid, "req": 16, "path": "src"})).await;
    assert_eq!(dir["kind"], "fs_deleted");
    assert!(!p.join("src").exists());
    // A path that leaves the project is an error.
    let out = ask(&mut ws, json!({"op": "fs_delete", "sid": sid, "req": 11, "path": "../x"})).await;
    assert_eq!(out["kind"], "fs_error");
}

#[tokio::test]
async fn the_trash_is_purged_and_emptied_only_after_the_summary_is_confirmed() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let sid = open_chat(&fx, &mut ws).await;
    let p = &fx.project;
    std::fs::create_dir_all(p.join("tree/sub")).unwrap();
    std::fs::write(p.join("tree/sub/f.txt"), "abc").unwrap();
    let gone = ask(&mut ws, json!({"op": "fs_delete", "sid": sid, "req": 1, "path": "tree"})).await;
    let trashed = gone["data"]["trashed"].as_str().unwrap().to_owned();
    // Without the confirmation: a summary only.
    let summary = ask(&mut ws, json!({"op": "fs_purge", "sid": sid, "req": 2, "path": trashed})).await;
    assert_eq!(summary["kind"], "fs_purge_summary");
    assert_eq!((summary["data"]["items"].as_u64(), summary["data"]["bytes"].as_u64()), (Some(3), Some(3)));
    assert!(p.join(&trashed).join("sub/f.txt").exists());
    let done = ask(&mut ws, json!({"op": "fs_purge", "sid": sid, "req": 3, "path": trashed, "confirm": true})).await;
    assert_eq!((done["kind"].as_str(), done["data"]["removed"].as_u64(), done["data"]["left"].as_u64()), (Some("fs_purged"), Some(3), Some(0)));
    assert!(!p.join(&trashed).exists());
    // Emptying: the same two steps.
    std::fs::write(p.join("a.txt"), "a").unwrap();
    std::fs::write(p.join("b.txt"), "bb").unwrap();
    for (req, path) in [(4, "a.txt"), (5, "b.txt")] {
        assert_eq!(ask(&mut ws, json!({"op": "fs_delete", "sid": sid, "req": req, "path": path})).await["kind"], "fs_deleted");
    }
    let summary = ask(&mut ws, json!({"op": "fs_empty_trash", "sid": sid, "req": 6, "path": ".tenrec-trash"})).await;
    assert_eq!((summary["kind"].as_str(), summary["data"]["entries"].as_u64()), (Some("fs_purge_summary"), Some(2)));
    let done = ask(&mut ws, json!({"op": "fs_empty_trash", "sid": sid, "req": 7, "path": ".tenrec-trash", "confirm": true})).await;
    assert_eq!((done["kind"].as_str(), done["data"]["removed"].as_u64()), (Some("fs_purged"), Some(2)));
    assert!(p.join(".tenrec-trash").is_dir());
    // What is not an entry of the trash is refused, and a restore that asks says where it would go.
    let bad = ask(&mut ws, json!({"op": "fs_purge", "sid": sid, "req": 8, "path": "src/main.rs", "confirm": true})).await;
    assert_eq!(bad["kind"], "fs_error");
    assert!(p.join("src/main.rs").exists());
    let claude = ask(&mut ws, json!({"op": "fs_delete", "sid": sid, "req": 9, "path": ".claude/settings.json", "confirm": true})).await;
    let in_trash = claude["data"]["trashed"].as_str().unwrap().to_owned();
    let asks = ask(&mut ws, json!({"op": "fs_restore", "sid": sid, "req": 10, "path": in_trash})).await;
    assert_eq!((asks["kind"].as_str(), asks["data"]["to"].as_str()), (Some("fs_needs_confirm"), Some(".claude/settings.json")));
}
