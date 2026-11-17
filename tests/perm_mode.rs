//! The permission mode chosen when a chat is made: what the host accepts, what it refuses, and what `claude` is started with.
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
    env_log: std::path::PathBuf,
    data: std::path::PathBuf,
}

async fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("scenario.jsonl"), "{\"$wait\":\"user\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}\n{\"$wait\":\"user\"}\n").unwrap();
    let env_log = root.join("env.log");
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_ENV_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), env_log.display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let data = root.join("data");
    let server = start(Config { port: 0, data_dir: data.clone(), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    Fx { _dir: dir, server, project, env_log, data }
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

/// The first `created` or `error` answer.
async fn answer(ws: &mut Ws) -> Value {
    loop {
        let got = next_json(ws).await;
        if got["kind"] == "created" || got["kind"] == "error" {
            return got;
        }
    }
}

async fn make(fx: &Fx, ws: &mut Ws, extra: Value) -> Value {
    let mut op = json!({"op": "new", "cwd": fx.project.display().to_string()});
    for (k, v) in extra.as_object().unwrap() {
        op[k] = v.clone();
    }
    ws.send(Message::Text(op.to_string())).await.unwrap();
    answer(ws).await
}

fn starts(fx: &Fx) -> Vec<Value> {
    std::fs::read_to_string(&fx.env_log).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

async fn send_and_wait(fx: &Fx, sid: &str) {
    let handle = fx.server.registry.get(sid).unwrap();
    let (_, mut rx) = handle.subscribe(0).await.unwrap();
    handle.send("hello".into(), vec![]).await.unwrap();
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.unwrap().unwrap();
        if got.kind == "state" && got.data["state"] == "idle" {
            break;
        }
    }
    handle.stop();
}

#[tokio::test]
async fn only_the_six_names_are_taken_and_skipping_every_check_needs_its_own_confirmation() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    assert_eq!(next_json(&mut ws).await["kind"], "hello");

    for bad in [json!("manual"), json!("yolo"), json!(""), json!("Plan"), json!(7), json!(true), json!(["plan"])] {
        let got = make(&fx, &mut ws, json!({"permission": bad})).await;
        assert_eq!(got["kind"], "error", "{bad}");
        assert_eq!(got["data"]["message"], "unknown permission mode", "{bad}");
    }
    let bare = make(&fx, &mut ws, json!({"permission": "bypassPermissions"})).await;
    assert_eq!(bare["kind"], "error");
    assert!(bare["data"]["message"].as_str().unwrap().contains("confirm_bypass"));
    let not_true = make(&fx, &mut ws, json!({"permission": "bypassPermissions", "confirm_bypass": "yes"})).await;
    assert_eq!(not_true["kind"], "error", "only a real true counts");
    assert!(fx.server.registry.list().await.is_empty(), "nothing was made by a refused request");

    let plain = make(&fx, &mut ws, json!({})).await;
    assert_eq!(plain["kind"], "created");
    assert!(plain["data"].get("permission_mode").is_none(), "a chat that never chose has nothing stored");
    let null = make(&fx, &mut ws, json!({"permission": null})).await;
    assert_eq!(null["kind"], "created");

    let accept = make(&fx, &mut ws, json!({"permission": "acceptEdits"})).await;
    assert_eq!(accept["kind"], "created");
    assert_eq!(accept["data"]["permission_mode"], "acceptEdits");

    // SAFETY: geteuid only reads the process's own id.
    if unsafe { libc::geteuid() } != 0 {
        let bypass = make(&fx, &mut ws, json!({"permission": "bypassPermissions", "confirm_bypass": true})).await;
        assert_eq!(bypass["kind"], "created");
        assert_eq!(bypass["data"]["permission_mode"], "bypassPermissions");
    }
}

#[tokio::test]
async fn claude_is_started_with_the_chosen_mode_every_time_and_an_ordinary_chat_with_none() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    assert_eq!(next_json(&mut ws).await["kind"], "hello");
    let plain = make(&fx, &mut ws, json!({})).await["sid"].as_str().unwrap().to_owned();
    let accept = make(&fx, &mut ws, json!({"permission": "acceptEdits"})).await["sid"].as_str().unwrap().to_owned();
    let plan = make(&fx, &mut ws, json!({"permission": "plan"})).await["sid"].as_str().unwrap().to_owned();

    send_and_wait(&fx, &plain).await;
    send_and_wait(&fx, &accept).await;
    send_and_wait(&fx, &plan).await;
    let all = starts(&fx);
    let argv = |i: usize| -> Vec<String> { all[i]["argv"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_owned()).collect() };
    assert!(!argv(0).iter().any(|a| a == "--permission-mode"), "the chat that never chose has the arguments it always had: {:?}", argv(0));
    assert!(argv(1).windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]), "{:?}", argv(1));
    assert!(argv(2).windows(2).any(|w| w == ["--permission-mode", "plan"]), "{:?}", argv(2));
    for i in 0..3 {
        assert!(argv(i).iter().any(|a| a == "--permission-prompt-tool"), "the person is still the one asked");
        assert!(!argv(i).iter().any(|a| a == "--dangerously-skip-permissions"));
    }

    // It is in the chat's file, so a restart (and a later `--resume`) starts with it again; a chat that never chose has no such key.
    let meta = |sid: &str| -> Value { serde_json::from_str(&std::fs::read_to_string(fx.data.join(format!("{sid}.meta.json"))).unwrap()).unwrap() };
    assert_eq!(meta(&accept)["permission_mode"], "acceptEdits");
    assert!(meta(&plain).get("permission_mode").is_none());
    // A file written before modes existed has no key and reads as the default.
    let old: tenrec::session::Meta = serde_json::from_value(meta(&plain)).unwrap();
    assert!(old.permission_mode.is_default());
    let again: tenrec::session::Meta = serde_json::from_value(meta(&accept)).unwrap();
    assert_eq!(again.permission_mode.name(), "acceptEdits");
}

#[tokio::test]
async fn a_name_the_list_does_not_know_in_a_chats_file_is_an_error_and_never_another_mode() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    assert_eq!(next_json(&mut ws).await["kind"], "hello");
    let sid = make(&fx, &mut ws, json!({"permission": "plan"})).await["sid"].as_str().unwrap().to_owned();
    let mut meta: Value = serde_json::from_str(&std::fs::read_to_string(fx.data.join(format!("{sid}.meta.json"))).unwrap()).unwrap();
    meta["permission_mode"] = json!("bypass-everything");
    assert!(serde_json::from_value::<tenrec::session::Meta>(meta).is_err());
}

#[test]
fn a_mode_fits_the_kind_of_chat_it_is_made_for() {
    use tenrec::claude::perm::PermMode;
    use tenrec::registry::{check_mode, mode_refusal};
    for m in PermMode::ALL.into_iter().filter(|m| *m != PermMode::Auto && *m != PermMode::Bypass) {
        assert!(check_mode(m, true, false).is_ok() && check_mode(m, false, false).is_ok(), "{m:?}");
    }
    assert_eq!(mode_refusal(PermMode::Auto, true, false), Some("perm_err_auto_provider"));
    assert_eq!(mode_refusal(PermMode::Auto, true, true), Some("perm_err_auto_provider"));
    assert!(check_mode(PermMode::Auto, false, false).is_ok());
    assert_eq!(mode_refusal(PermMode::Bypass, true, false), Some("perm_err_bypass_provider"));
    // SAFETY: geteuid only reads the process's own id.
    if unsafe { libc::geteuid() } != 0 {
        assert!(check_mode(PermMode::Bypass, true, true).is_ok(), "inside a container a provider chat may");
        assert!(check_mode(PermMode::Bypass, false, false).is_ok(), "a person's own chat may, after confirming");
    }
}

#[tokio::test]
async fn one_chat_file_that_cannot_be_read_hides_only_that_chat() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    assert_eq!(next_json(&mut ws).await["kind"], "hello");
    let good_sid = make(&fx, &mut ws, json!({"permission": "plan"})).await["sid"].as_str().unwrap().to_owned();
    let bad_sid = make(&fx, &mut ws, json!({"permission": "acceptEdits"})).await["sid"].as_str().unwrap().to_owned();
    let path = fx.data.join(format!("{bad_sid}.meta.json"));
    let mut meta: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    meta["permission_mode"] = json!("bypass-everything");
    std::fs::write(&path, meta.to_string()).unwrap();
    let Fx { _dir, server, project, env_log, data } = fx;
    drop(ws);
    drop(server);
    // A new host on the same folder.
    let wrapper = _dir.path().join("claude");
    let again = start(Config { port: 0, data_dir: data, bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let sids: Vec<String> = again.registry.list().await.into_iter().map(|i| i.meta.sid).collect();
    assert!(sids.contains(&good_sid) && !sids.contains(&bad_sid), "{sids:?}");
    let _ = (project, env_log);
}
