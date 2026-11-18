//! Extra accounts (a personal and a work login kept apart): the environment and arguments their `claude` gets, and that nothing of one
//! account's notes, skills or history reaches a chat, a page or a worker of another one, in both directions.
#![allow(clippy::result_large_err)]
use std::path::PathBuf;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tenrec::extract::ExtractConfig;
use tenrec::index::SearchQuery;
use tenrec::memory::NewMem;
use tenrec::propose::ProposeConfig;
use tenrec::server::{start, Config, Running};
use tenrec::session::SessionHandle;
use tenrec::settings::Auto;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

/// One turn that runs three commands (enough for an automatic skill proposal to be considered).
const CHAT: &str = r#"{"$wait":"user"}
{"type":"system","subtype":"init","session_id":"A1"}
{"type":"assistant","parent_tool_use_id":null,"message":{"id":"a1","content":[{"type":"text","text":"done"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"make one"}},{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"make two"}},{"type":"tool_use","id":"t3","name":"Bash","input":{"command":"make three"}}]}}
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
    fixture_with(None).await
}

/// `auth` is the text `claude auth status` prints (`$DIR` is the folder it was given).
async fn fixture_with(auth: Option<&str>) -> Fx {
    build(auth, Duration::from_millis(150), 1).await
}

/// Workers that wait `wait` after a turn and read only after `batch` turns: long enough to do something in between.
async fn build(auth: Option<&str>, wait: Duration, batch: u32) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("scenario.jsonl"), CHAT).unwrap();
    let (log, env_log) = (root.join("fake.log"), root.join("env.log"));
    let auth_env = match auth {
        Some(text) => {
            std::fs::write(root.join("auth.json"), text).unwrap();
            format!("FAKE_AUTH='{}' ", root.join("auth.json").display())
        }
        None => String::new(),
    };
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\n{auth_env}FAKE_SCENARIO='{}' FAKE_LOG='{}' FAKE_ENV_LOG='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), log.display(), env_log.display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let extract = ExtractConfig { debounce: wait, batch_turns: batch, daily_cap: 40, retries: 0 };
    let propose = ProposeConfig { debounce: wait, min_tools: 3, daily_cap: 5 };
    let server = start(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: Some(PathBuf::from(env!("CARGO_BIN_EXE_tenrec"))), extract, propose, sched: Default::default() }).await.unwrap();
    Fx { _dir: dir, root, server, log, env_log }
}

fn project(fx: &Fx, name: &str) -> PathBuf {
    let path = fx.root.join(name);
    std::fs::create_dir_all(&path).unwrap();
    path.canonicalize().unwrap()
}

fn work(fx: &Fx) -> String {
    fx.server.registry.add_account("Work", tenrec::accounts::AccountKind::Claude).unwrap().id
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

/// The starts the fake logged, `auth` calls left out.
fn started(fx: &Fx) -> Vec<Value> {
    std::fs::read_to_string(&fx.env_log).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["argv"][0] != "auth").collect()
}

fn argv(start: &Value) -> Vec<String> {
    start["argv"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_owned()).collect()
}

fn note_for(realm: &tenrec::realm::Realm, cwd: &std::path::Path, text: &str, scope: &'static str) {
    let added = realm.memory.clone().unwrap().add(NewMem { scope, cwd: (scope == "project").then(|| cwd.display().to_string()), kind: "fact".into(), text: text.into(), source: "manual", sid: None, seq: None, status: "active", instr: false, allow_secret: false });
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

// ---- the environment and arguments ----

#[tokio::test]
async fn an_account_chat_runs_in_its_own_settings_folder_without_the_default_accounts_credentials_and_with_its_own_notes_and_search() {
    let fx = fixture().await;
    let id = work(&fx);
    let folder = project(&fx, "a");
    let realm = fx.server.registry.realm(Some(&id)).unwrap();
    note_for(&realm, &folder, "WORKNOTE the deploy runs on friday", "project");
    let chat = fx.server.registry.create_in(folder.to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    assert_eq!(chat.info().await.unwrap().meta.account.as_ref().map(|a| a.id.clone()), Some(id.clone()));
    turn(&chat, "hello").await;

    let runs = started(&fx);
    assert_eq!(runs.len(), 1, "one process was started");
    let (args, env) = (argv(&runs[0]), &runs[0]["env"]);
    let config = fx.root.join("data").join("accounts").join(&id).canonicalize().unwrap();
    assert_eq!(env["CLAUDE_CONFIG_DIR"], json!(config.display().to_string()), "the chat has the account's own settings folder");
    for credential in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL", "CLAUDE_CODE_OAUTH_TOKEN"] {
        assert!(env.get(credential).is_none(), "{credential} of the default account is not given");
    }
    for given in ["--mcp-config", "--append-system-prompt-file"] {
        assert!(args.iter().any(|a| a == given), "{given} is given: the account's own notes and history search work");
    }
    for banned in ["--settings", "--setting-sources"] {
        assert!(!args.iter().any(|a| a == banned), "{banned} is for a provider chat only: the person's own project settings apply");
    }
    let (missed, _) = chat.subscribe(0).await.unwrap();
    assert!(missed.iter().any(|e| e.kind == "memory_used"), "the account's note was given to its chat");
}

// ---- both directions: notes, search, skills, auto-save ----

#[tokio::test]
async fn notes_do_not_cross_between_accounts_in_either_direction_even_in_the_same_folder() {
    let fx = fixture().await;
    let id = work(&fx);
    let folder = project(&fx, "same");
    let (default, work_realm) = (fx.server.registry.realm(None).unwrap(), fx.server.registry.realm(Some(&id)).unwrap());
    note_for(&default, &folder, "PERSONALNOTE about the home server", "project");
    note_for(&work_realm, &folder, "WORKNOTE about the deploy", "project");
    note_for(&default, &folder, "PERSONALUSER I like tabs", "user");
    note_for(&work_realm, &folder, "WORKUSER I answer in english", "user");

    let personal_chat = fx.server.registry.create(folder.to_str().unwrap(), None, None).unwrap();
    let work_chat = fx.server.registry.create_in(folder.to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    // The recall file is read while the process lives: the fake logs the file's name only, so read what each realm would hand over.
    let seen_by = |realm: &tenrec::realm::Realm| -> String { realm.memory.clone().unwrap().recall(&folder.to_string_lossy()).into_iter().map(|n| n.text).collect::<Vec<_>>().join("\n") };
    let (personal, work) = (seen_by(&default), seen_by(&work_realm));
    assert!(personal.contains("PERSONALNOTE") && personal.contains("PERSONALUSER") && !personal.contains("WORK"), "{personal}");
    assert!(work.contains("WORKNOTE") && work.contains("WORKUSER") && !work.contains("PERSONAL"), "{work}");

    // The chats were given exactly those: the number of notes each was told it got.
    turn(&personal_chat, "hello").await;
    turn(&work_chat, "hello").await;
    for chat in [&personal_chat, &work_chat] {
        let (events, _) = chat.subscribe(0).await.unwrap();
        let used = events.iter().find(|e| e.kind == "memory_used").expect("notes were given");
        assert_eq!(used.data["ids"].as_array().unwrap().len(), 2, "each chat got its own account's two notes and nothing else");
    }
}

#[tokio::test]
async fn history_does_not_cross_between_accounts_in_the_index_the_pages_search_or_the_titles() {
    let fx = fixture().await;
    let id = work(&fx);
    let folder = project(&fx, "same");
    let personal_chat = fx.server.registry.create(folder.to_str().unwrap(), None, None).unwrap();
    let work_chat = fx.server.registry.create_in(folder.to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    turn(&personal_chat, "PERSONALNEEDLE please").await;
    turn(&work_chat, "WORKNEEDLE please").await;
    let (default, work_realm) = (fx.server.registry.realm(None).unwrap(), fx.server.registry.realm(Some(&id)).unwrap());
    let search = |realm: &tenrec::realm::Realm, q: &str| {
        let index = realm.index.clone().unwrap();
        index.flush();
        index.search(&SearchQuery { q: q.into(), cwd: None, sid: None, since: None, limit: 10 }).unwrap()
    };
    assert!(!search(&default, "PERSONALNEEDLE").is_empty() && search(&default, "WORKNEEDLE").is_empty(), "the default index holds only the default account's chats");
    assert!(!search(&work_realm, "WORKNEEDLE").is_empty() && search(&work_realm, "PERSONALNEEDLE").is_empty(), "the account's index holds only its own chats");
    assert_ne!(default.index.as_ref().map(std::sync::Arc::as_ptr), work_realm.index.as_ref().map(std::sync::Arc::as_ptr));
    assert!(fx.root.join("data").join("stores").join(&id).join("index.sqlite").is_file(), "the account's files are in its own folder");
}

#[tokio::test]
async fn a_realm_that_cannot_be_found_is_an_error_and_never_the_default_one() {
    let fx = fixture().await;
    assert!(fx.server.registry.realm(Some("0000000000000000")).is_err());
    assert!(fx.server.registry.realm(Some("")).is_err());
    assert!(fx.server.registry.create_in(project(&fx, "a").to_str().unwrap(), None, None, false, Some("0000000000000000")).is_err(), "a chat cannot be made for an account that does not exist");
}

#[tokio::test]
async fn auto_save_and_proposals_of_an_account_run_as_that_account_and_read_only_its_chats() {
    let fx = fixture().await;
    let id = work(&fx);
    fx.server.registry.settings().update(|s| {
        s.memory_auto = Auto::Project;
        s.skills_auto = true;
    });
    let folder = project(&fx, "same");
    let personal_chat = fx.server.registry.create(folder.to_str().unwrap(), None, None).unwrap();
    let work_chat = fx.server.registry.create_in(folder.to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    turn(&work_chat, "WORKTEXT please").await;
    wait_for("the account's two runs", || {
        let log = std::fs::read_to_string(&fx.log).unwrap_or_default();
        log.contains("EXTRACT_IN") && log.contains("PROPOSE_IN")
    })
    .await;
    turn(&personal_chat, "PERSONALTEXT please").await;
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let log = std::fs::read_to_string(&fx.log).unwrap();
    let runs: Vec<&str> = log.lines().filter(|l| l.starts_with("EXTRACT_IN") || l.starts_with("PROPOSE_IN")).collect();
    assert!(runs.iter().any(|l| l.contains("WORKTEXT")) && runs.iter().any(|l| l.contains("PERSONALTEXT")), "both accounts' chats were read:\n{runs:#?}");
    // Each run was started as the account whose chat it read: a work text only in a run with the work settings folder.
    let config = fx.root.join("data").join("accounts").join(&id).canonicalize().unwrap().display().to_string();
    let env_runs: Vec<Value> = std::fs::read_to_string(&fx.env_log).unwrap().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| argv(v).iter().any(|a| a == "--json-schema")).collect();
    assert!(env_runs.len() >= 4, "two runs per account: {}", env_runs.len());
    let as_work = env_runs.iter().filter(|v| v["env"]["CLAUDE_CONFIG_DIR"] == json!(config)).count();
    assert_eq!(as_work, 2, "the work account's runs (auto-save and proposal) used its settings folder, and the default account's did not");
    assert_eq!(env_runs.iter().filter(|v| v["env"].get("CLAUDE_CONFIG_DIR").is_none()).count(), 2, "the default account's runs have no folder set");
    // The chats' own readers refuse the other account's.
    assert!(work_chat.proposal_input(0).await.unwrap().account.is_some());
    assert!(personal_chat.proposal_input(0).await.unwrap().account.is_none());
}

#[tokio::test]
async fn the_accounts_notes_live_in_its_own_file_and_a_damaged_file_affects_only_that_account() {
    let fx = fixture().await;
    let id = work(&fx);
    let default_count = fx.server.registry.realm(None).unwrap().memory.clone().unwrap().counts();
    note_for(&fx.server.registry.realm(Some(&id)).unwrap(), &project(&fx, "a"), "WORKNOTE", "user");
    assert_eq!(fx.server.registry.realm(None).unwrap().memory.clone().unwrap().counts(), default_count, "nothing was added to the default account's notes");
    let own = fx.root.join("data").join("stores").join(&id).join("memory.sqlite");
    assert!(own.is_file());
    // Damaged on disk, reopened: that account has no notes, and the default one is not given in their place.
    let data = fx.root.join("data");
    drop(fx.server);
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(own.with_extension(format!("sqlite{suffix}")));
    }
    std::fs::write(&own, vec![b'x'; 8192]).unwrap();
    let again = start(Config { port: 0, data_dir: data, bin: None, mcp_bin: None, extract: ExtractConfig::default(), propose: ProposeConfig::default(), sched: Default::default() }).await.unwrap();
    let realm = again.registry.realm(Some(&id)).unwrap();
    // The store sets a file it cannot read aside and starts empty, saying so: the account has no notes (not the default account's).
    assert!(realm.memory_notice.is_some(), "the person is told");
    assert_eq!(realm.memory.clone().unwrap().counts(), (0, 0), "no notes, and none borrowed from the default account");
    assert!(again.registry.realm(None).unwrap().memory_notice.is_none(), "the default account is not affected");
}

// ---- the page's messages ----

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn socket(fx: &Fx) -> Ws {
    let port = fx.server.addr.port();
    let mut request = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
    request.headers_mut().insert("Origin", HeaderValue::from_str(&format!("http://127.0.0.1:{port}")).unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    ws.send(Message::Text(json!({"op": "auth", "token": fx.server.token}).to_string())).await.unwrap();
    ws
}

async fn next_text(ws: &mut Ws, kinds: &[&str]) -> Value {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(10), ws.next()).await.expect("timed out").expect("closed").expect("error");
        if let Message::Text(text) = got {
            let v: Value = serde_json::from_str(&text).unwrap();
            if kinds.iter().any(|k| v["kind"] == *k) {
                return v;
            }
        }
    }
}

/// Sends an op and waits for the reply of `kind` (matched by `rid` when the reply has one).
async fn call(ws: &mut Ws, op: Value, kind: &str) -> Value {
    let rid = op["rid"].clone();
    ws.send(Message::Text(op.to_string())).await.unwrap();
    loop {
        let v = next_text(ws, &[kind]).await;
        if rid.is_null() || v["data"]["rid"] == rid {
            return v["data"].clone();
        }
    }
}

#[tokio::test]
async fn a_message_about_notes_skills_or_search_must_name_its_account_and_acts_on_that_one_only() {
    let fx = fixture().await;
    let id = work(&fx);
    let mut ws = socket(&fx).await;

    // No account key: refused, for each of the three panels.
    assert_eq!(call(&mut ws, json!({"op": "memory_add", "rid": 1, "text": "x", "scope": "user"}), "memory_result").await["ok"], false);
    assert_eq!(call(&mut ws, json!({"op": "skill_approve", "rid": 2, "id": 1}), "skill_result").await["ok"], false);
    let refused = call(&mut ws, json!({"op": "search", "req": 3, "q": "x"}), "search_results").await;
    assert!(refused["error"].as_str().unwrap().contains("account"), "{refused}");
    // An unknown id is refused, not answered from the default account.
    let unknown = call(&mut ws, json!({"op": "memory_add", "rid": 4, "account": "ffffffffffffffff", "text": "x", "scope": "user"}), "memory_result").await;
    assert_eq!(unknown["ok"], false);
    assert_eq!(fx.server.registry.realm(None).unwrap().memory.clone().unwrap().counts().0, 0, "nothing reached the default account");

    // With the key, the note lands in that account's file and in no other.
    let added = call(&mut ws, json!({"op": "memory_add", "rid": 5, "account": id, "text": "WORKONLY", "scope": "user"}), "memory_result").await;
    assert_eq!(added["ok"], true, "{added}");
    assert_eq!(fx.server.registry.realm(Some(&id)).unwrap().memory.clone().unwrap().counts().0, 1);
    assert_eq!(fx.server.registry.realm(None).unwrap().memory.clone().unwrap().counts().0, 0);
    // Each list shows its own account's notes and says which account it is.
    let work_list = call(&mut ws, json!({"op": "memory_list", "account": id}), "memory").await;
    let default_list = call(&mut ws, json!({"op": "memory_list", "account": null}), "memory").await;
    assert!(work_list.to_string().contains("WORKONLY") && work_list["account"] == json!(id), "{work_list}");
    assert!(!default_list.to_string().contains("WORKONLY") && default_list["account"].is_null(), "{default_list}");
}

#[tokio::test]
async fn a_search_that_names_a_chat_of_another_account_is_refused() {
    let fx = fixture().await;
    let id = work(&fx);
    let folder = project(&fx, "a");
    let work_chat = fx.server.registry.create_in(folder.to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    turn(&work_chat, "WORKNEEDLE").await;
    let mut ws = socket(&fx).await;
    let refused = call(&mut ws, json!({"op": "search", "req": 1, "q": "WORKNEEDLE", "sid": work_chat.sid, "account": null}), "search_results").await;
    assert!(refused["hits"].as_array().unwrap().is_empty() && refused["error"].is_string(), "{refused}");
    let found = call(&mut ws, json!({"op": "search", "req": 2, "q": "WORKNEEDLE", "account": id}), "search_results").await;
    assert!(!found["hits"].as_array().unwrap().is_empty(), "{found}");
    let wrong = call(&mut ws, json!({"op": "search", "req": 3, "q": "WORKNEEDLE", "account": null}), "search_results").await;
    assert!(wrong["hits"].as_array().unwrap().is_empty(), "the default account's search does not find the account's chat: {wrong}");
}

// ---- combinations, login and removal ----

#[tokio::test]
async fn an_account_cannot_be_mixed_with_a_provider_or_a_container() {
    let fx = fixture().await;
    let id = work(&fx);
    let provider = fx.server.registry.providers().save(tenrec::providers::Form { name: "GW".into(), base_url: "https://gw.example.com/x".into(), token: Some("sk-test-token-9999".into()), ..Default::default() }, 1).unwrap();
    let folder = project(&fx, "a");
    assert!(fx.server.registry.create_in(folder.to_str().unwrap(), None, Some(&provider), false, Some(&id)).is_err());
    assert!(fx.server.registry.create_in(folder.to_str().unwrap(), None, None, true, Some(&id)).is_err());
}

#[tokio::test]
async fn a_chat_of_a_logged_out_account_does_not_start_and_says_how_to_log_in() {
    let fx = fixture_with(Some(r#"{"loggedIn":false,"configDirectory":"$DIR"}"#)).await;
    let id = work(&fx);
    let chat = fx.server.registry.create_in(project(&fx, "a").to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    let error = chat.send("hello".into(), vec![]).await.unwrap_err();
    assert!(error.contains("not logged in"), "{error}");
    assert!(started(&fx).is_empty(), "no chat process was started");
}

#[tokio::test]
async fn a_status_that_reports_another_settings_folder_is_refused() {
    let fx = fixture_with(Some(r#"{"loggedIn":true,"configDirectory":"/somewhere/else"}"#)).await;
    let id = work(&fx);
    let chat = fx.server.registry.create_in(project(&fx, "a").to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    let error = chat.send("hello".into(), vec![]).await.unwrap_err();
    assert!(error.contains("another settings folder"), "{error}");
    assert!(started(&fx).is_empty());
}

#[tokio::test]
async fn a_logged_in_account_chat_starts_and_a_default_chat_never_asks_for_a_status() {
    let fx = fixture().await;
    let id = work(&fx);
    let account_chat = fx.server.registry.create_in(project(&fx, "a").to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    let default_chat = fx.server.registry.create(project(&fx, "b").to_str().unwrap(), None, None).unwrap();
    turn(&account_chat, "hello").await;
    turn(&default_chat, "hello").await;
    let lines: Vec<Value> = std::fs::read_to_string(&fx.env_log).unwrap().lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    assert_eq!(lines.iter().filter(|v| v["argv"][0] == "auth").count(), 1, "only the account's chat asked");
}

#[tokio::test]
async fn the_folder_is_looked_at_again_at_every_start_and_a_changed_one_stops_the_chat() {
    let fx = fixture().await;
    let id = work(&fx);
    let chat = fx.server.registry.create_in(project(&fx, "a").to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let config = fx.root.join("data").join("accounts").join(&id);
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = chat.send("hello".into(), vec![]).await.unwrap_err();
    assert!(error.contains("not safe") || error.contains("missing"), "{error}");
    assert!(started(&fx).is_empty());
}

#[tokio::test]
async fn removing_an_account_needs_a_confirmation_signs_it_out_and_removes_its_data_but_leaves_its_chats_readable() {
    let fx = fixture().await;
    let id = work(&fx);
    let folder = project(&fx, "a");
    let chat = fx.server.registry.create_in(folder.to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    turn(&chat, "WORKNEEDLE").await;
    let mut ws = socket(&fx).await;
    let refused = call(&mut ws, json!({"op": "account_delete", "rid": 1, "id": id}), "account_result").await;
    assert_eq!(refused["ok"], false);
    assert!(fx.root.join("data").join("stores").join(&id).is_dir(), "nothing was removed without the confirmation");
    let done = call(&mut ws, json!({"op": "account_delete", "rid": 2, "id": id, "confirm": true}), "account_result").await;
    assert_eq!(done["ok"], true, "{done}");
    assert!(done["warnings"].as_array().unwrap().is_empty(), "{done}");
    assert!(!fx.root.join("data").join("stores").join(&id).exists() && !fx.root.join("data").join("accounts").join(&id).exists());
    assert!(fx.server.registry.realm(Some(&id)).is_err());
    assert!(std::fs::read_to_string(&fx.log).unwrap().contains("AUTH_LOGOUT"), "the program was asked to log the account out");
    // The chat stays readable and cannot be started again.
    let (events, _) = chat.subscribe(0).await.unwrap();
    assert!(events.iter().any(|e| e.kind == "user"));
    assert!(chat.send("again".into(), vec![]).await.is_err());
}

#[tokio::test]
async fn the_panel_lists_accounts_with_a_login_command_for_the_persons_own_terminal() {
    let fx = fixture().await;
    let mut ws = socket(&fx).await;
    let added = call(&mut ws, json!({"op": "account_add", "rid": 1, "name": "Work"}), "account_result").await;
    assert_eq!(added["ok"], true, "{added}");
    let id = added["account"]["id"].as_str().unwrap().to_owned();
    let login = added["login"].as_str().unwrap();
    assert!(login.starts_with("CLAUDE_CONFIG_DIR='") && login.ends_with("' claude auth login"), "{login}");
    assert!(login.contains(&fx.root.join("data").join("accounts").join(&id).display().to_string()));
    assert_eq!(call(&mut ws, json!({"op": "account_add", "rid": 2, "name": "Work"}), "account_result").await["ok"], false, "a name is used once");
    let list = call(&mut ws, json!({"op": "account_list"}), "accounts").await;
    assert_eq!(list["accounts"].as_array().unwrap().len(), 1);
    assert!(!list.to_string().contains("token"), "no token is anywhere in the panel's data");
    let status = call(&mut ws, json!({"op": "account_status", "req": 3, "id": id}), "account_status").await;
    assert_eq!(status["status"]["logged_in"], true, "{status}");
    assert_eq!(call(&mut ws, json!({"op": "account_rename", "rid": 4, "id": id, "name": "Company"}), "account_result").await["ok"], true);
}

#[tokio::test]
async fn an_account_is_still_there_after_a_restart_and_its_chat_resumes_in_the_same_folder() {
    let fx = fixture().await;
    let id = work(&fx);
    let folder = project(&fx, "a");
    let chat = fx.server.registry.create_in(folder.to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    turn(&chat, "one").await;
    let (sid, data) = (chat.sid.clone(), fx.root.join("data"));
    let wrapper = fx.root.join("claude");
    drop(fx.server);
    let again = start(Config { port: 0, data_dir: data.clone(), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: ExtractConfig::default(), propose: ProposeConfig::default(), sched: Default::default() }).await.unwrap();
    let chat = again.registry.get(&sid).expect("the chat came back");
    assert_eq!(chat.info().await.unwrap().meta.account.as_ref().map(|a| a.id.clone()), Some(id.clone()));
    assert!(again.registry.realm(Some(&id)).is_ok());
}

#[tokio::test]
async fn a_chat_made_before_accounts_existed_reads_as_the_default_account_and_writes_no_account_key() {
    let fx = fixture().await;
    let chat = fx.server.registry.create(project(&fx, "a").to_str().unwrap(), None, None).unwrap();
    let meta = std::fs::read_to_string(fx.root.join("data").join(format!("{}.meta.json", chat.sid))).unwrap();
    assert!(!meta.contains("account"), "{meta}");
    let parsed: tenrec::session::Meta = serde_json::from_str(&meta).unwrap();
    assert!(parsed.account.is_none());
}

#[tokio::test]
async fn skills_do_not_cross_between_accounts_in_either_direction_even_in_the_same_folder() {
    let fx = fixture().await;
    let id = work(&fx);
    let folder = project(&fx, "same");
    let cwd = folder.to_string_lossy().into_owned();
    let approve = |realm: std::sync::Arc<tenrec::realm::Realm>, name: &str| {
        let store = realm.skills.clone().unwrap();
        let proposal = store
            .propose(tenrec::skills::NewProposal {
                target: None,
                name: name.into(),
                scope: "project",
                cwd: Some(cwd.clone()),
                description: "Runs the project's checks. Use when the user asks to run the checklist.".into(),
                body: "1. Run cargo test --all.".into(),
                sid: None,
                seq: None,
            })
            .unwrap();
        store.approve(proposal.id, Default::default(), false).unwrap();
    };
    approve(fx.server.registry.realm(None).unwrap(), "personal-check");
    approve(fx.server.registry.realm(Some(&id)).unwrap(), "work-check");

    let given = |chat: SessionHandle| async move {
        let (_, mut rx) = chat.subscribe(0).await.unwrap();
        chat.send("hello".into(), vec![]).await.unwrap();
        loop {
            let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.unwrap().unwrap();
            if got.kind == "skills_given" {
                return got.data["names"].clone();
            }
        }
    };
    let personal = given(fx.server.registry.create(&cwd, None, None).unwrap()).await;
    let worked = given(fx.server.registry.create_in(&cwd, None, None, false, Some(&id)).unwrap()).await;
    assert_eq!(personal, json!(["personal-check"]), "the default account's chat gets its own skill only");
    assert_eq!(worked, json!(["work-check"]), "the account's chat gets its own skill only");
}

#[tokio::test]
async fn an_entry_of_another_account_in_an_index_is_removed_at_the_next_start() {
    let fx = fixture().await;
    let id = work(&fx);
    let folder = project(&fx, "a");
    let work_chat = fx.server.registry.create_in(folder.to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    turn(&work_chat, "WORKNEEDLE").await;
    // The work chat's history is put into the default index (as an older build, or a hand edit, could have).
    let (events, _) = work_chat.subscribe(0).await.unwrap();
    let default_index = fx.server.registry.realm(None).unwrap().index.clone().unwrap();
    default_index.catch_up(&work_chat.sid, &folder.to_string_lossy(), 1, events);
    default_index.flush();
    let found = |index: &tenrec::index::Index| index.search(&SearchQuery { q: "WORKNEEDLE".into(), cwd: None, sid: None, since: None, limit: 10 }).unwrap().len();
    assert!(found(&default_index) > 0, "the setup put it there");
    let (data, wrapper) = (fx.root.join("data"), fx.root.join("claude"));
    drop(default_index);
    drop(fx.server);
    let again = start(Config { port: 0, data_dir: data, bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: ExtractConfig::default(), propose: ProposeConfig::default(), sched: Default::default() }).await.unwrap();
    let index = again.registry.realm(None).unwrap().index.clone().unwrap();
    index.flush();
    assert_eq!(found(&index), 0, "the default index no longer holds the account's chat");
    let own = again.registry.realm(Some(&id)).unwrap().index.clone().unwrap();
    own.flush();
    assert!(found(&own) > 0, "the account's own index still has it");
}

#[tokio::test]
async fn a_worker_skips_a_chat_of_another_account() {
    let fx = fixture().await;
    let id = work(&fx);
    fx.server.registry.settings().update(|s| {
        s.memory_auto = Auto::Project;
        s.skills_auto = true;
    });
    let folder = project(&fx, "a");
    let work_chat = fx.server.registry.create_in(folder.to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    turn(&work_chat, "WORKFORGED please").await;
    wait_for("the account's own runs", || std::fs::read_to_string(&fx.log).unwrap_or_default().contains("EXTRACT_IN")).await;
    let before = std::fs::read_to_string(&fx.log).unwrap().lines().filter(|l| l.starts_with("EXTRACT_IN")).count();
    // The default account's channel is told about the work chat (a mistake somewhere upstream): its worker must not read it.
    let default = fx.server.registry.realm(None).unwrap();
    default.turns.as_ref().unwrap().send(tenrec::extract::TurnEnd { sid: work_chat.sid.clone(), cwd: folder.to_string_lossy().into_owned() }).unwrap();
    default.skill_turns.as_ref().unwrap().send(tenrec::extract::TurnEnd { sid: work_chat.sid.clone(), cwd: folder.to_string_lossy().into_owned() }).unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let log = std::fs::read_to_string(&fx.log).unwrap();
    assert_eq!(log.lines().filter(|l| l.starts_with("EXTRACT_IN")).count(), before, "the default account's worker did not read the work chat:\n{log}");
    let proposals = log.lines().filter(|l| l.starts_with("PROPOSE_IN")).count();
    assert!(proposals <= 1, "at most the account's own automatic proposal ran: {proposals}");
}

#[tokio::test]
async fn a_removed_account_cannot_be_started_again_even_when_its_folder_could_not_be_removed_and_its_workers_read_nothing() {
    // The workers wait 700 ms after a turn, so the account can be removed while a turn of it is still waiting to be read.
    let fx = build(None, Duration::from_millis(700), 10).await;
    let id = work(&fx);
    fx.server.registry.settings().update(|s| {
        s.memory_auto = Auto::Project;
        s.skills_auto = true;
    });
    let folder = project(&fx, "a");
    let chat = fx.server.registry.create_in(folder.to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    turn(&chat, "UNREADWORKTEXT please").await;
    let started_before = started(&fx).len();
    // The parent folder cannot be written to: the account's folders cannot be removed, which is the worst case for the rest.
    use std::os::unix::fs::PermissionsExt;
    let accounts = fx.root.join("data").join("accounts");
    std::fs::set_permissions(&accounts, std::fs::Permissions::from_mode(0o500)).unwrap();
    let warnings = fx.server.registry.delete_account(&id).await.unwrap();
    std::fs::set_permissions(&accounts, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(warnings.contains(&"folder_left"), "{warnings:?}");
    assert!(accounts.join(&id).is_dir(), "the folder is still there");

    let error = chat.send("hello".into(), vec![]).await.unwrap_err();
    assert!(error.contains("not available"), "{error}");
    assert_eq!(started(&fx).len(), started_before, "no process was started for the removed account");
    assert!(fx.server.registry.create_in(folder.to_str().unwrap(), None, None, false, Some(&id)).is_err(), "no new chat either");

    // The workers woke up after the account was removed, with a turn nobody had read, and read none of it.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let log = std::fs::read_to_string(&fx.log).unwrap_or_default();
    let runs: Vec<&str> = log.lines().filter(|l| l.starts_with("EXTRACT_IN") || l.starts_with("PROPOSE_IN")).collect();
    assert!(runs.is_empty(), "a worker of the removed account read its chat:\n{runs:#?}");
}

#[tokio::test]
async fn a_chat_with_a_running_process_is_refused_once_its_account_is_retired() {
    // A send that races the removal of the account must not start a turn under it, also when `claude` is already running.
    let fx = fixture().await;
    let id = work(&fx);
    let chat = fx.server.registry.create_in(project(&fx, "a").to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    turn(&chat, "hello").await;
    let started_before = started(&fx).len();
    fx.server.registry.realm(Some(&id)).unwrap().set_retired(true);
    let error = chat.send("again".into(), vec![]).await.unwrap_err();
    assert!(error.contains("not available"), "{error}");
    assert_eq!(started(&fx).len(), started_before, "no turn was started");
}

#[tokio::test]
async fn the_control_for_the_removal_test_the_same_waiting_workers_do_read_a_chat_of_an_account_that_stays() {
    let fx = build(None, Duration::from_millis(700), 10).await;
    let id = work(&fx);
    fx.server.registry.settings().update(|s| {
        s.memory_auto = Auto::Project;
        s.skills_auto = true;
    });
    let chat = fx.server.registry.create_in(project(&fx, "a").to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    turn(&chat, "UNREADWORKTEXT please").await;
    wait_for("the workers' runs", || std::fs::read_to_string(&fx.log).unwrap_or_default().lines().any(|l| l.starts_with("EXTRACT_IN") && l.contains("UNREADWORKTEXT"))).await;
}

#[tokio::test]
async fn after_a_restart_the_chat_of_a_removed_account_is_readable_and_does_not_start_as_the_default_account() {
    let fx = fixture().await;
    let id = work(&fx);
    let chat = fx.server.registry.create_in(project(&fx, "a").to_str().unwrap(), None, None, false, Some(&id)).unwrap();
    turn(&chat, "hello").await;
    let sid = chat.sid.clone();
    fx.server.registry.delete_account(&id).await.unwrap();
    let (before, env_log) = (started(&fx).len(), fx.env_log.clone());
    let (data, wrapper) = (fx.root.join("data"), fx.root.join("claude"));
    drop(fx.server);
    let again = start(Config { port: 0, data_dir: data, bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: ExtractConfig::default(), propose: ProposeConfig::default(), sched: Default::default() }).await.unwrap();
    let chat = again.registry.get(&sid).expect("the chat is still listed");
    let (events, _) = chat.subscribe(0).await.unwrap();
    assert!(events.iter().any(|e| e.kind == "user"), "its history can be read");
    assert!(chat.send("again".into(), vec![]).await.is_err());
    let now = std::fs::read_to_string(&env_log).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["argv"][0] != "auth").count();
    assert_eq!(now, before, "nothing was started, not even as the default account");
    // Its words are in no index.
    let found = |realm: std::sync::Arc<tenrec::realm::Realm>| {
        let index = realm.index.clone().unwrap();
        index.flush();
        index.search(&SearchQuery { q: "hello".into(), cwd: None, sid: None, since: None, limit: 10 }).unwrap().len()
    };
    assert_eq!(found(again.registry.realm(None).unwrap()), 0);
}

#[tokio::test]
async fn a_change_event_names_its_account_so_a_page_showing_another_one_does_not_redraw() {
    let fx = fixture().await;
    let id = work(&fx);
    let mut ws = socket(&fx).await;
    let mut heard = fx.server.registry.events.subscribe();
    let added = call(&mut ws, json!({"op": "memory_add", "rid": 1, "account": id, "text": "WORKEVENT", "scope": "user"}), "memory_result").await;
    assert_eq!(added["ok"], true, "{added}");
    let event = loop {
        let e = tokio::time::timeout(Duration::from_secs(5), heard.recv()).await.unwrap().unwrap();
        if e["kind"] == "memory_changed" {
            break e;
        }
    };
    assert_eq!(event["data"]["account"], json!(id), "{event}");
    call(&mut ws, json!({"op": "memory_add", "rid": 2, "account": null, "text": "PERSONALEVENT", "scope": "user"}), "memory_result").await;
    let event = loop {
        let e = tokio::time::timeout(Duration::from_secs(5), heard.recv()).await.unwrap().unwrap();
        if e["kind"] == "memory_changed" {
            break e;
        }
    };
    assert!(event["data"]["account"].is_null(), "{event}");
}
