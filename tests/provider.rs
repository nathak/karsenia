//! Chats made against another provider: the environment and arguments their `claude` gets, that the key is shown nowhere, that nothing
//! of the account's side of the app (notes, skills, history search, auto-save, proposals) crosses to them or back, and that a chat
//! whose provider is gone or moved does not start.
#![allow(clippy::result_large_err)]
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tenrec::extract::ExtractConfig;
use tenrec::index::SearchQuery;
use tenrec::memory::NewMem;
use tenrec::propose::ProposeConfig;
use tenrec::providers::Form;
use tenrec::server::{start, Config, Running};
use tenrec::session::SessionHandle;
use tenrec::settings::Auto;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");
const KEY: &str = "sk-test-token-9999";

/// One turn that says the key out loud and runs three commands (enough for an automatic skill proposal to be considered).
const CHAT: &str = r#"{"$wait":"user"}
{"type":"system","subtype":"init","session_id":"P1"}
{"type":"assistant","parent_tool_use_id":null,"message":{"id":"a1","content":[{"type":"text","text":"the key is sk-test-token-9999 ok"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"make one"}},{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"make two"}},{"type":"tool_use","id":"t3","name":"Bash","input":{"command":"make three"}}]}}
{"type":"result","subtype":"success","is_error":false}
{"$wait":"user"}
"#;

struct Fx {
    _dir: tempfile::TempDir,
    root: PathBuf,
    server: Running,
    log: PathBuf,
    env_log: PathBuf,
}

async fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("scenario.jsonl"), CHAT).unwrap();
    let (log, env_log) = (root.join("fake.log"), root.join("env.log"));
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' FAKE_ENV_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), log.display(), env_log.display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let extract = ExtractConfig { debounce: Duration::from_millis(150), batch_turns: 1, daily_cap: 40, retries: 0 };
    let propose = ProposeConfig { debounce: Duration::from_millis(150), min_tools: 3, daily_cap: 5 };
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: Some(PathBuf::from(env!("CARGO_BIN_EXE_tenrec"))), extract, propose, sched: Default::default() }).await.unwrap();
    Fx { _dir: dir, root, server, log, env_log }
}

fn project(fx: &Fx, name: &str) -> PathBuf {
    let path = fx.root.join(name);
    std::fs::create_dir_all(&path).unwrap();
    path.canonicalize().unwrap()
}

fn gateway(fx: &Fx, name: &str) -> String {
    fx.server
        .registry
        .providers()
        .save(Form { name: name.into(), base_url: "https://gw.example.com/x".into(), token: Some(KEY.into()), model: "gw-model".into(), ..Default::default() }, 1)
        .unwrap()
}

async fn turn(session: &SessionHandle, text: &str) {
    let (_, mut rx) = session.subscribe(0).await.unwrap();
    session.send(text.into(), vec![]).await.unwrap();
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.unwrap().unwrap();
        if got.kind == "state" && got.data["state"] == "idle" {
            return;
        }
    }
}

fn started(fx: &Fx) -> Vec<Value> {
    std::fs::read_to_string(&fx.env_log).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn argv(start: &Value) -> Vec<String> {
    start["argv"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_owned()).collect()
}

/// Every file under `dir`, but `skip`, as text.
fn tree_text(dir: &Path, skip: &str) -> String {
    let mut all = String::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            all.push_str(&tree_text(&path, skip));
        } else if path.file_name().and_then(|n| n.to_str()) != Some(skip) {
            all.push_str(&String::from_utf8_lossy(&std::fs::read(&path).unwrap_or_default()));
        }
    }
    all
}

fn note(fx: &Fx, cwd: &Path) {
    let added = fx.server.registry.realm(None).unwrap().memory.clone().unwrap().add(NewMem { scope: "project", cwd: Some(cwd.display().to_string()), kind: "fact".into(), text: "The release script is scripts/ship.sh".into(), source: "manual", sid: None, seq: None, status: "active", instr: false, allow_secret: false });
    assert!(matches!(added, tenrec::memory::Added::Saved(_)), "{added:?}");
}

async fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..100 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
async fn a_provider_chat_runs_with_its_own_key_in_its_own_settings_folder_and_without_the_accounts_side_of_the_app() {
    let fx = fixture().await;
    let folder = project(&fx, "a");
    note(&fx, &folder);
    let id = gateway(&fx, "Gateway");
    let chat = fx.server.registry.create(folder.to_str().unwrap(), None, Some(&id)).unwrap();
    turn(&chat, "hello").await;

    let runs = started(&fx);
    assert_eq!(runs.len(), 1, "one process was started");
    let (args, env) = (argv(&runs[0]), &runs[0]["env"]);
    assert_eq!(env["ANTHROPIC_BASE_URL"], json!("https://gw.example.com/x"));
    assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], json!(KEY));
    let config = fx.root.join("data").join("chatcfg").join(&chat.sid);
    assert_eq!(env["CLAUDE_CONFIG_DIR"], json!(config.display().to_string()), "the chat has a settings folder of its own, not the account's");
    assert!(config.is_dir());
    assert_eq!(env["CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"], json!("1"));
    assert!(args.windows(2).any(|w| w == ["--setting-sources", "user"]), "project settings are not read: {args:?}");
    let pinned = args.windows(2).find(|w| w[0] == "--settings").map(|w| w[1].clone()).expect("the address is pinned");
    assert_eq!(serde_json::from_str::<Value>(&pinned).unwrap(), json!({"env": {"ANTHROPIC_BASE_URL": "https://gw.example.com/x"}}));
    assert!(args.windows(2).any(|w| w == ["--model", "gw-model"]), "the provider's own default model is used");
    for banned in ["--mcp-config", "--append-system-prompt-file", "--plugin-dir"] {
        assert!(!args.iter().any(|a| a == banned), "{banned} would carry the account's notes, skills or history search to a third party");
    }
    assert!(!args.join(" ").contains(KEY), "the key is never an argument");

    // The key is shown and stored nowhere: it was said by the model, and it is replaced.
    let data = fx.root.join("data");
    let stored = tree_text(&data, "providers.json");
    assert!(!stored.contains(KEY), "the key is in a file of the data folder");
    assert!(stored.contains("[provider key]"), "the chat shows where it was");
    let (missed, _) = chat.subscribe(0).await.unwrap();
    assert!(!serde_json::to_string(&missed).unwrap().contains(KEY));
    assert!(!missed.iter().any(|e| e.kind == "memory_used"), "no notes were given to a provider chat");
    let info = serde_json::to_string(&chat.info().await.unwrap()).unwrap();
    assert!(info.contains("gw.example.com") && !info.contains(KEY) && !info.contains("/x\""), "the page learns the host and not the key or the path: {info}");
}

#[tokio::test]
async fn a_normal_chat_is_as_it_was() {
    let fx = fixture().await;
    let folder = project(&fx, "a");
    note(&fx, &folder);
    gateway(&fx, "Gateway");
    let chat = fx.server.registry.create(folder.to_str().unwrap(), None, None).unwrap();
    turn(&chat, "hello").await;
    let runs = started(&fx);
    let (args, env) = (argv(&runs[0]), &runs[0]["env"]);
    assert!(args.iter().any(|a| a == "--append-system-prompt-file"), "notes are given to a normal chat");
    assert!(args.iter().any(|a| a == "--mcp-config"), "history search is given to a normal chat, so its absence from a provider chat means something");
    for absent in ["--settings", "--setting-sources"] {
        assert!(!args.iter().any(|a| a == absent), "{absent} is only for a provider chat");
    }
    assert!(env.get("CLAUDE_CONFIG_DIR").is_none_or(|v| !v.as_str().unwrap_or_default().contains("chatcfg")));
    assert!(!env.to_string().contains(KEY));
    assert!(!fx.root.join("data").join("chatcfg").exists());
    let (missed, _) = chat.subscribe(0).await.unwrap();
    assert!(missed.iter().any(|e| e.kind == "memory_used"));
}

#[tokio::test]
async fn a_provider_chat_is_not_read_for_notes_skills_or_search_and_a_normal_chat_still_is() {
    let fx = fixture().await;
    let registry = &fx.server.registry;
    registry.settings().update(|s| {
        s.memory_auto = Auto::Project;
        s.skills_auto = true;
    });
    let id = gateway(&fx, "Gateway");
    let provider_chat = registry.create(project(&fx, "a").to_str().unwrap(), None, Some(&id)).unwrap();
    turn(&provider_chat, "PROVIDERNEEDLE please").await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let log = std::fs::read_to_string(&fx.log).unwrap_or_default();
    assert!(!log.contains("EXTRACT_IN") && !log.contains("PROPOSE_IN"), "a run of the account read a provider chat:\n{log}");

    // Each layer on its own: what auto-save would read is empty, and what a proposal would read is marked as a provider chat's.
    assert!(provider_chat.user_texts(0, 0).await.unwrap().is_empty(), "a provider chat's messages are handed to auto-save");
    assert!(provider_chat.proposal_input(0).await.unwrap().provider);

    // Not in the history index either: the account's chats search it.
    let index = registry.realm(None).unwrap().index.clone().unwrap();
    index.flush();
    let search = |q: &str| index.search(&SearchQuery { q: q.into(), cwd: None, sid: None, since: None, limit: 10 }).unwrap();
    assert!(search("PROVIDERNEEDLE").is_empty(), "a provider chat's words are in the history index");

    // The same settings and the same turn in a normal chat do start both runs: the checks above can fail.
    let normal = registry.create(project(&fx, "b").to_str().unwrap(), None, None).unwrap();
    turn(&normal, "NORMALNEEDLE please").await;
    wait_for("the account's runs on a normal chat", || {
        let log = std::fs::read_to_string(&fx.log).unwrap_or_default();
        log.contains("EXTRACT_IN") && log.contains("PROPOSE_IN")
    })
    .await;
    index.flush();
    assert!(!search("NORMALNEEDLE").is_empty());
    assert!(!normal.user_texts(0, 0).await.unwrap().is_empty(), "the control: a normal chat's messages are handed to auto-save");
    assert!(!normal.proposal_input(0).await.unwrap().provider);
    let log = std::fs::read_to_string(&fx.log).unwrap();
    let account_runs: String = log.lines().filter(|l| l.starts_with("EXTRACT_IN") || l.starts_with("PROPOSE_IN")).collect::<Vec<_>>().join("\n");
    assert!(account_runs.contains("NORMALNEEDLE") && !account_runs.contains("PROVIDERNEEDLE"), "what the account's runs read holds the provider chat's words:\n{account_runs}");
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn socket(fx: &Fx) -> Ws {
    let port = fx.server.addr.port();
    let mut request = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
    request.headers_mut().insert("Origin", HeaderValue::from_str(&format!("http://127.0.0.1:{port}")).unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    ws.send(Message::Text(json!({"op": "auth", "token": fx.server.token}).to_string())).await.unwrap();
    ws
}

async fn next_text(ws: &mut Ws, kinds: &[&str]) -> (Value, String) {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(10), ws.next()).await.expect("timed out").expect("closed").expect("error");
        if let Message::Text(text) = got {
            let v: Value = serde_json::from_str(&text).unwrap();
            if kinds.iter().any(|k| v["kind"] == *k) {
                return (v, text.to_string());
            }
        }
    }
}

async fn ask(ws: &mut Ws, op: Value) -> Value {
    let rid = op["rid"].clone();
    ws.send(Message::Text(op.to_string())).await.unwrap();
    loop {
        let (v, _) = next_text(ws, &["provider_result"]).await;
        if v["data"]["rid"] == rid {
            return v["data"].clone();
        }
    }
}

#[tokio::test]
async fn the_providers_panel_takes_a_key_and_never_gives_it_back_and_a_new_chat_is_bound_to_one() {
    let fx = fixture().await;
    let mut ws = socket(&fx).await;
    for (what, patch) in [("an http address", json!({"base_url": "http://example.com"})), ("a user name in the address", json!({"base_url": "https://u@example.com"})), ("no name", json!({"name": ""})), ("a key with a space", json!({"token": "a b"}))] {
        let mut form = json!({"op": "provider_save", "rid": 1, "name": "GW", "base_url": "https://gw.example.com/x", "auth": "bearer", "token": KEY});
        for (k, v) in patch.as_object().unwrap() {
            form[k] = v.clone();
        }
        assert_eq!(ask(&mut ws, form).await["ok"], json!(false), "{what} must be refused");
    }
    let saved = ask(&mut ws, json!({"op": "provider_save", "rid": 2, "name": "GW", "base_url": "https://gw.example.com/x", "auth": "bearer", "token": KEY})).await;
    assert_eq!(saved["ok"], json!(true), "{saved}");
    let id = saved["id"].as_str().unwrap().to_owned();

    ws.send(Message::Text(json!({"op": "provider_list"}).to_string())).await.unwrap();
    let (listing, raw) = next_text(&mut ws, &["providers"]).await;
    assert!(!raw.contains(KEY), "the list holds the key: {raw}");
    assert_eq!((listing["data"]["providers"][0]["has_token"].clone(), listing["data"]["providers"][0]["host"].clone(), listing["data"]["providers"][0]["chats"].clone()), (json!(true), json!("gw.example.com"), json!(0)));

    let folder = project(&fx, "a");
    ws.send(Message::Text(json!({"op": "new", "cwd": folder.display().to_string(), "provider": "nope"}).to_string())).await.unwrap();
    next_text(&mut ws, &["error"]).await;
    ws.send(Message::Text(json!({"op": "new", "cwd": folder.display().to_string(), "provider": id}).to_string())).await.unwrap();
    let (created, raw) = next_text(&mut ws, &["created"]).await;
    assert!(!raw.contains(KEY));
    assert_eq!(created["data"]["provider"]["host"], json!("gw.example.com"));
    assert_eq!(created["data"]["provider"]["name"], json!("GW"));

    ws.send(Message::Text(json!({"op": "provider_list"}).to_string())).await.unwrap();
    assert_eq!(next_text(&mut ws, &["providers"]).await.0["data"]["providers"][0]["chats"], json!(1), "the panel counts the chats bound to a provider");

    // A skill proposal for a provider chat is refused: its run would go to the account.
    let sid = created["sid"].as_str().unwrap().to_owned();
    ws.send(Message::Text(json!({"op": "skill_propose", "account": null, "rid": 9, "sid": sid, "hint": "a hint"}).to_string())).await.unwrap();
    let (notice, _) = next_text(&mut ws, &["skill_notice"]).await;
    assert!(notice["data"]["text"].as_str().unwrap().contains("another provider"), "{notice}");
}

#[tokio::test]
async fn a_chat_whose_provider_was_deleted_or_moved_does_not_start_and_a_changed_key_does_not_matter() {
    let fx = fixture().await;
    let folder = project(&fx, "a");
    let store = fx.server.registry.providers();
    let id = gateway(&fx, "Gateway");

    let moved = fx.server.registry.create(folder.to_str().unwrap(), None, Some(&id)).unwrap();
    store.save(Form { id: Some(id.clone()), name: "Gateway".into(), base_url: "https://other.example.com/x".into(), ..Default::default() }, 2).unwrap();
    let err = moved.send("hello".into(), vec![]).await.unwrap_err();
    assert!(err.contains("address") && err.contains("changed"), "{err}");

    store.save(Form { id: Some(id.clone()), name: "Gateway".into(), base_url: "https://gw.example.com/other-path".into(), token: Some("sk-new-key-0000".into()), ..Default::default() }, 3).unwrap();
    turn(&moved, "hello").await;
    let runs = started(&fx);
    assert_eq!(runs[0]["env"]["ANTHROPIC_AUTH_TOKEN"], json!("sk-new-key-0000"), "a changed key and path on the same host are used by the next start");

    let orphan = fx.server.registry.create(folder.to_str().unwrap(), None, Some(&id)).unwrap();
    assert_eq!(store.delete(&id), Ok(true));
    let err = orphan.send("hello".into(), vec![]).await.unwrap_err();
    assert!(err.contains("deleted"), "{err}");
    assert_eq!(started(&fx).len(), 1, "no process was started for the chat without a provider");
    assert!(!tree_text(&fx.root.join("data"), "providers.json").contains("sk-new-key-0000"));
}

#[tokio::test]
async fn a_chat_keeps_its_provider_after_a_restart_and_is_still_not_in_the_history_index() {
    let fx = fixture().await;
    let id = gateway(&fx, "Gateway");
    let chat = fx.server.registry.create(project(&fx, "a").to_str().unwrap(), None, Some(&id)).unwrap();
    turn(&chat, "RESTARTPROVIDERWORD").await;
    let sid = chat.sid.clone();
    // The same words in a normal chat are what make the check able to fail.
    let normal = fx.server.registry.create(project(&fx, "b").to_str().unwrap(), None, None).unwrap();
    turn(&normal, "RESTARTNORMALWORD").await;

    let wrapper = fx.root.join("claude").display().to_string();
    let reopened = tenrec::registry::Registry::open(fx.root.join("data"), Some(wrapper)).unwrap();
    let info = reopened.get(&sid).unwrap().info().await.unwrap();
    assert_eq!(info.meta.provider.unwrap().host, "gw.example.com");
    // The chat goes on after the restart: what it says now stays out of the index too (its session was given no index).
    let again = reopened.get(&sid).unwrap();
    turn(&again, "RESTARTAFTERWORD").await;
    let normal_again = reopened.get(&normal.sid).unwrap();
    turn(&normal_again, "RESTARTNORMALAFTER").await;
    let index = reopened.realm(None).unwrap().index.clone().unwrap();
    index.flush();
    let search = |q: &str| index.search(&SearchQuery { q: q.into(), cwd: None, sid: None, since: None, limit: 10 }).unwrap();
    assert!(!search("RESTARTNORMALWORD").is_empty(), "a normal chat is brought back into the index at a restart");
    assert!(search("RESTARTPROVIDERWORD").is_empty(), "a provider chat is brought into the index at a restart");
    assert!(!search("RESTARTNORMALAFTER").is_empty(), "the control: a normal chat that goes on after a restart is indexed");
    assert!(search("RESTARTAFTERWORD").is_empty(), "a provider chat that goes on after a restart is written into the index");
}
