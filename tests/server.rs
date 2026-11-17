//! The server end to end with a fake `claude`: who may connect, the permission round trip over the WebSocket, image files.
#![allow(clippy::result_large_err)]
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tenrec::server::{start, Config, Running};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

const SCENARIO: &str = r#"{"$wait":"user"}
{"type":"system","subtype":"init","session_id":"W1"}
{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"touch a"},"permission_suggestions":[]}}
{"$wait":"control_response"}
{"type":"result","subtype":"success","is_error":false}
{"$wait":"user"}
"#;

struct Fx {
    _dir: tempfile::TempDir,
    server: Running,
    project: std::path::PathBuf,
    log: std::path::PathBuf,
}

async fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let scenario = root.join("scenario.jsonl");
    std::fs::write(&scenario, SCENARIO).unwrap();
    let log = root.join("fake.log");
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", scenario.display(), log.display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();

    Fx { _dir: dir, server, project, log }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(fx: &Fx, origin: Option<&str>) -> Result<Ws, tokio_tungstenite::tungstenite::Error> {
    let port = fx.server.addr.port();
    let mut request = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
    if let Some(origin) = origin {
        request.headers_mut().insert("Origin", HeaderValue::from_str(origin).unwrap());
    }
    tokio_tungstenite::connect_async(request).await.map(|(ws, _)| ws)
}

async fn good(fx: &Fx) -> Ws {
    let origin = format!("http://127.0.0.1:{}", fx.server.addr.port());
    let mut ws = connect(fx, Some(&origin)).await.unwrap();
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

#[tokio::test]
async fn only_this_servers_own_pages_get_in() {
    let fx = fixture().await;
    let port = fx.server.addr.port();
    let http = reqwest::Client::new();

    let page = http.get(format!("http://127.0.0.1:{port}/")).send().await.unwrap();
    assert_eq!(page.status(), 200);
    assert!(page.headers()["content-security-policy"].to_str().unwrap().contains("default-src 'self'"));
    assert_eq!(page.headers()["x-content-type-options"], "nosniff");

    let rebinding = http.get(format!("http://127.0.0.1:{port}/")).header("Host", "evil.test").send().await.unwrap();
    assert_eq!(rebinding.status(), 421);

    assert!(connect(&fx, None).await.is_err(), "no Origin");
    assert!(connect(&fx, Some("http://evil.test")).await.is_err(), "foreign Origin");

    let origin = format!("http://127.0.0.1:{port}");
    let mut wrong = connect(&fx, Some(&origin)).await.unwrap();
    wrong.send(Message::Text(json!({"op": "auth", "token": "nope"}).to_string())).await.unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match wrong.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return true,
                Some(Ok(Message::Text(_))) => return false,
                _ => {}
            }
        }
    })
    .await;
    assert_eq!(ended, Ok(true), "a wrong token gets no data");

    let mut silent = connect(&fx, Some(&origin)).await.unwrap();
    silent.send(Message::Text(json!({"op": "list"}).to_string())).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), silent.next()).await;
    assert!(!matches!(first, Ok(Some(Ok(Message::Text(_))))), "the first message must be the token");
}

#[tokio::test]
async fn a_permission_goes_to_the_browser_and_waits_for_its_answer() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    assert_eq!(next_json(&mut ws).await["kind"], "hello");

    ws.send(Message::Text(json!({"op": "new", "cwd": fx.project, "model": "haiku"}).to_string())).await.unwrap();
    let created = until_kind(&mut ws, "created").await;
    let sid = created["sid"].as_str().unwrap().to_owned();
    until_kind(&mut ws, "attached").await;

    ws.send(Message::Text(json!({"op": "send", "sid": sid, "text": "go"}).to_string())).await.unwrap();
    let ask = until_kind(&mut ws, "permission").await;
    assert_eq!(ask["data"]["request_id"], "r1");

    // The page goes away; the request stays open and nothing is answered on its behalf.
    drop(ws);
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(!std::fs::read_to_string(&fx.log).unwrap().contains("control_response"));

    // A new page attaches from the start, sees the same request, and answers it.
    let mut again = good(&fx).await;
    until_kind(&mut again, "hello").await;
    again.send(Message::Text(json!({"op": "attach", "sid": sid, "after_seq": 0}).to_string())).await.unwrap();
    let replayed = until_kind(&mut again, "permission").await;
    assert_eq!(replayed["data"]["request_id"], "r1");
    again.send(Message::Text(json!({"op": "permission", "sid": sid, "request_id": "r1", "allow": true, "input": {"command": "rm -rf /"}}).to_string())).await.unwrap();
    until_kind(&mut again, "permission_resolved").await;
    loop {
        let e = next_json(&mut again).await;
        if e["kind"] == "state" && e["data"]["state"] == "idle" {
            break;
        }
    }
    let log = std::fs::read_to_string(&fx.log).unwrap();
    assert!(log.contains(r#""command":"touch a""#) && !log.contains("rm -rf"), "the original input goes back, never one the page sent");
}

#[tokio::test]
async fn image_files_need_the_token_and_stay_inside_the_project() {
    let fx = fixture().await;
    let port = fx.server.addr.port();
    std::fs::write(fx.project.join("shot.png"), b"\x89PNG fake").unwrap();
    let outside = fx.project.parent().unwrap().join("outside.png");
    std::fs::write(&outside, b"x").unwrap();

    let mut ws = good(&fx).await;
    until_kind(&mut ws, "hello").await;
    ws.send(Message::Text(json!({"op": "new", "cwd": fx.project}).to_string())).await.unwrap();
    let sid = until_kind(&mut ws, "created").await["sid"].as_str().unwrap().to_owned();

    let http = reqwest::Client::new();
    let url = |path: &str| format!("http://127.0.0.1:{port}/api/file?sid={sid}&path={path}");
    let bearer = format!("Bearer {}", fx.server.token);

    assert_eq!(http.get(url("shot.png")).send().await.unwrap().status(), 401);
    assert_eq!(http.get(url("shot.png")).header("Authorization", "Bearer wrong").send().await.unwrap().status(), 401);
    let ok = http.get(url("shot.png")).header("Authorization", &bearer).send().await.unwrap();
    assert_eq!(ok.status(), 200);
    assert_eq!(ok.headers()["content-type"], "image/png");
    assert_eq!(http.get(url("..%2Foutside.png")).header("Authorization", &bearer).send().await.unwrap().status(), 404);
    assert_eq!(http.get(url("missing.png")).header("Authorization", &bearer).send().await.unwrap().status(), 404);
    let absolute = url(&outside.display().to_string().replace('/', "%2F"));
    assert_eq!(http.get(absolute).header("Authorization", &bearer).send().await.unwrap().status(), 404);
}

#[tokio::test]
async fn the_memory_panel_adds_edits_approves_and_undoes_and_every_page_hears_of_it() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    until_kind(&mut ws, "hello").await;
    let mut other = good(&fx).await;
    until_kind(&mut other, "hello").await;
    let cwd = fx.project.canonicalize().unwrap().to_string_lossy().into_owned();
    let send = |v: Value| Message::Text(v.to_string());

    ws.send(send(json!({"op": "memory_add", "account": null, "text": "I use pnpm", "scope": "project", "cwd": cwd}))).await.unwrap();
    let added = until_kind(&mut ws, "memory_result").await;
    assert_eq!(added["data"]["ok"], true);
    let id = added["data"]["saved"]["id"].as_i64().unwrap();
    assert_eq!(until_kind(&mut other, "memory_changed").await["kind"], "memory_changed", "another page hears of the change");

    ws.send(send(json!({"op": "memory_add", "account": null, "text": "I use pnpm", "scope": "project", "cwd": cwd}))).await.unwrap();
    assert_eq!(until_kind(&mut ws, "memory_result").await["data"]["ok"], false, "the same note twice");
    ws.send(send(json!({"op": "memory_add", "account": null, "text": "password: hunter2", "scope": "project", "cwd": cwd}))).await.unwrap();
    let refused = until_kind(&mut ws, "memory_result").await;
    assert_eq!(refused["data"]["needs_confirm"], true, "a secret-like note needs the person to confirm");
    ws.send(send(json!({"op": "memory_add", "account": null, "text": "password: hunter2", "scope": "project", "cwd": cwd, "confirm": true}))).await.unwrap();
    assert_eq!(until_kind(&mut ws, "memory_result").await["data"]["ok"], true);
    ws.send(send(json!({"op": "memory_add", "account": null, "text": "no folder given", "scope": "project"}))).await.unwrap();
    assert_eq!(until_kind(&mut ws, "memory_result").await["data"]["ok"], false);

    ws.send(send(json!({"op": "memory_edit", "account": null, "id": id, "text": "I use yarn"}))).await.unwrap();
    assert_eq!(until_kind(&mut ws, "memory_result").await["data"]["ok"], true);
    ws.send(send(json!({"op": "memory_pin", "account": null, "id": id, "pinned": true}))).await.unwrap();
    assert_eq!(until_kind(&mut ws, "memory_result").await["data"]["ok"], true);
    ws.send(send(json!({"op": "memory_list", "account": null, "cwd": cwd}))).await.unwrap();
    let list = until_kind(&mut ws, "memory").await;
    assert_eq!(list["data"]["overview"]["project"][0]["text"], "I use yarn");
    assert_eq!(list["data"]["overview"]["project"][0]["pinned"], true);
    assert_eq!(list["data"]["settings"]["memory_auto"], "off", "auto-save starts off");

    ws.send(send(json!({"op": "memory_delete", "account": null, "id": id}))).await.unwrap();
    assert_eq!(until_kind(&mut ws, "memory_result").await["data"]["ok"], true);
    ws.send(send(json!({"op": "memory_undo", "account": null, "id": id}))).await.unwrap();
    assert_eq!(until_kind(&mut ws, "memory_result").await["data"]["ok"], true);

    ws.send(send(json!({"op": "memory_setting", "auto": "project"}))).await.unwrap();
    until_kind(&mut ws, "memory_result").await;
    ws.send(send(json!({"op": "memory_setting", "auto": "everything"}))).await.unwrap();
    until_kind(&mut ws, "memory_result").await;
    ws.send(send(json!({"op": "memory_list", "account": null}))).await.unwrap();
    assert_eq!(until_kind(&mut ws, "memory").await["data"]["settings"]["memory_auto"], "project", "an unknown value changes nothing");

    ws.send(send(json!({"op": "memory_export", "account": null}))).await.unwrap();
    let export = until_kind(&mut ws, "memory_export").await;
    assert!(export["data"]["notes"].as_array().unwrap().len() >= 2);
    assert!(!ws_text_has_token(&export, &fx.server.token));
}

fn ws_text_has_token(value: &Value, token: &str) -> bool {
    value.to_string().contains(token)
}
