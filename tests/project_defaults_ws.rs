//! What the new-chat form remembers for a project: recorded when a chat is made from the form, answered by `project_defaults`, never
//! the mode that skips every check, never a chat on a provider.
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


async fn ask(ws: &mut Ws, cwd: &str) -> Value {
    ws.send(Message::Text(json!({"op": "project_defaults", "cwd": cwd}).to_string())).await.unwrap();
    loop {
        let got = next_json(ws).await;
        if got["kind"] == "project_defaults" {
            return got["data"].clone();
        }
    }
}

#[tokio::test]
async fn a_chat_made_from_the_form_is_what_the_next_form_for_that_project_starts_with() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    assert_eq!(next_json(&mut ws).await["kind"], "hello");
    let cwd = fx.project.display().to_string();
    assert_eq!(ask(&mut ws, &cwd).await["found"], false, "nothing is known of a project that was never made");

    let made = make(&fx, &mut ws, json!({"model": "haiku", "permission": "acceptEdits"})).await;
    assert_eq!(made["kind"], "created", "{made}");
    let got = ask(&mut ws, &cwd).await;
    assert_eq!((got["found"].clone(), got["model"].clone(), got["permission"].clone(), got["worktree"].clone()), (json!(true), json!("haiku"), json!("acceptEdits"), json!(false)), "{got}");
    assert_eq!(got["account"], Value::Null, "the default account is null, not missing");
    assert_eq!(got["cwd"], cwd, "the answer says which folder it is for, so a late one can be told");
    assert!(fx.data.join("project_defaults.json").exists());

    // The same folder reached another way is the same project.
    let slash = format!("{cwd}/");
    assert_eq!(ask(&mut ws, &slash).await["model"], "haiku");
}

#[tokio::test]
async fn skipping_every_check_is_never_remembered_and_does_not_bring_the_earlier_mode_back() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    assert_eq!(next_json(&mut ws).await["kind"], "hello");
    let cwd = fx.project.display().to_string();
    assert_eq!(make(&fx, &mut ws, json!({"permission": "plan"})).await["kind"], "created");
    assert_eq!(ask(&mut ws, &cwd).await["permission"], "plan");
    assert_eq!(make(&fx, &mut ws, json!({"permission": "bypassPermissions", "confirm_bypass": true})).await["kind"], "created");
    let got = ask(&mut ws, &cwd).await;
    assert_eq!(got["found"], true);
    assert!(got.get("permission").is_none(), "no mode is filled in at all: {got}");
}

#[tokio::test]
async fn a_folder_that_is_not_there_or_not_absolute_is_not_found() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    assert_eq!(next_json(&mut ws).await["kind"], "hello");
    for cwd in ["relative/path", "", "/definitely/not/a/folder"] {
        assert_eq!(ask(&mut ws, cwd).await["found"], false, "{cwd:?}");
    }
}

#[tokio::test]
async fn a_chat_that_failed_to_start_records_nothing() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    assert_eq!(next_json(&mut ws).await["kind"], "hello");
    let cwd = fx.project.display().to_string();
    let refused = make(&fx, &mut ws, json!({"permission": "bypassPermissions"})).await;
    assert_eq!(refused["kind"], "error");
    assert_eq!(ask(&mut ws, &cwd).await["found"], false);
}

#[tokio::test]
async fn a_chat_on_a_provider_or_in_a_container_is_not_remembered_and_a_worktree_chat_is_filed_under_its_project() {
    use tenrec::providers::ProviderRef;
    use tenrec::session::new_meta;
    let fx = fixture().await;
    let cwd = fx.project.display().to_string();
    let provider = ProviderRef { id: "p1".into(), name: "Third".into(), host: "api.example".into(), login: false };
    fx.server.registry.remember_defaults(&new_meta(fx.project.clone(), None, Some(provider), false, None));
    fx.server.registry.remember_defaults(&new_meta(fx.project.clone(), None, None, true, None));
    assert_eq!(fx.server.registry.project_defaults(&cwd)["found"], false, "neither is what the next chat starts with");

    let mut meta = new_meta(fx._dir.path().join("elsewhere"), Some("opus".into()), None, false, None);
    meta.worktree = Some(tenrec::worktree::WorktreeRef { repo: fx.project.clone(), branch: "tenrec/x".into(), base_branch: "main".into(), base_commit: "c".into(), removed: false, root: None, config_hash: String::new() });
    fx.server.registry.remember_defaults(&meta);
    let got = fx.server.registry.project_defaults(&cwd);
    assert_eq!((got["found"].clone(), got["worktree"].clone(), got["model"].clone()), (json!(true), json!(true), json!("opus")), "a worktree chat is of the folder it was made from: {got}");
}

#[tokio::test]
async fn an_account_that_is_gone_is_not_named_to_the_form() {
    use tenrec::accounts::AccountRef;
    use tenrec::session::new_meta;
    let fx = fixture().await;
    let cwd = fx.project.display().to_string();
    let account = AccountRef { id: "0123456789abcdef".into(), name: "Gone".into() };
    fx.server.registry.remember_defaults(&new_meta(fx.project.clone(), None, None, false, Some(account)));
    let got = fx.server.registry.project_defaults(&cwd);
    assert_eq!(got["found"], true);
    assert!(got.get("account").is_none(), "the form keeps its own account: {got}");
}
