//! The Schedule tab's operations over the WebSocket: what a task may be made of, "run now", and what the page is shown.
#![allow(clippy::result_large_err)]
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use swb_app::server::{start, Config, Running};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

struct Fx {
    _dir: tempfile::TempDir,
    server: Running,
    project: std::path::PathBuf,
    data: std::path::PathBuf,
}

async fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_LOG='{}' exec '{}' \"$@\"\n", root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let data = root.join("data");
    let server = start(Config { port: 0, data_dir: data.clone(), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    Fx { _dir: dir, server, project, data }
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

async fn until_kind(ws: &mut Ws, kind: &str) -> Value {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(10), ws.next()).await.expect("timed out").expect("closed").expect("error");
        if let Message::Text(text) = got {
            let v: Value = serde_json::from_str(&text).unwrap();
            if v["kind"] == kind {
                return v;
            }
        }
    }
}

async fn ask(ws: &mut Ws, op: Value) -> Value {
    let rid = op["rid"].clone();
    ws.send(Message::Text(op.to_string())).await.unwrap();
    loop {
        let got = until_kind(ws, "sched_result").await;
        if got["data"]["rid"] == rid {
            return got["data"].clone();
        }
    }
}

fn form(fx: &Fx, rid: u64) -> Value {
    json!({"op": "sched_save", "rid": rid, "name": "Morning", "prompt": "Summarise the README.", "profile": "folder", "cwd": fx.project.display().to_string(), "kind": "daily", "minute_of_day": 540, "weekdays": 31})
}

async fn list(ws: &mut Ws) -> Value {
    ws.send(Message::Text(json!({"op": "sched_list"}).to_string())).await.unwrap();
    until_kind(ws, "sched").await["data"].clone()
}

#[tokio::test]
async fn a_task_is_made_checked_changed_and_deleted_by_the_person_and_bad_forms_are_refused() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let first = list(&mut ws).await;
    assert_eq!((first["available"].clone(), first["on"].clone(), first["tasks"].as_array().unwrap().len()), (json!(true), json!(false), 0), "the scheduler starts off");

    let bad = |patch: Value| {
        let mut f = form(&fx, 1);
        for (k, v) in patch.as_object().unwrap() {
            f[k] = v.clone();
        }
        f
    };
    for (what, patch) in [
        ("no profile", json!({"profile": ""})),
        ("a profile that is both", json!({"profile": "both"})),
        ("the data folder", json!({"cwd": fx.data.display().to_string()})),
        ("a folder that holds the data folder", json!({"cwd": fx.data.parent().unwrap().display().to_string()})),
        ("a relative folder", json!({"cwd": "project"})),
        ("root", json!({"cwd": "/"})),
        ("an interval under 15 minutes", json!({"kind": "every", "every_min": 5})),
        ("no weekday", json!({"weekdays": 0})),
        ("a time of day out of range", json!({"minute_of_day": 1440})),
        ("no schedule", json!({"kind": "never"})),
        ("an empty instruction", json!({"prompt": "  "})),
        ("a huge budget", json!({"budget_usd": 100})),
        ("a short time limit", json!({"timeout_s": 5})),
        ("a bad model name", json!({"model": "a b"})),
        ("a model name that is a flag", json!({"model": "--dangerously-skip-permissions"})),
    ] {
        let answer = ask(&mut ws, bad(patch)).await;
        assert_eq!(answer["ok"], json!(false), "{what} must be refused: {answer}");
    }
    assert_eq!(list(&mut ws).await["tasks"].as_array().unwrap().len(), 0, "nothing was made by the refused forms");

    let made = ask(&mut ws, form(&fx, 2)).await;
    assert_eq!(made["ok"], json!(true), "{made}");
    let id = made["id"].as_i64().unwrap();
    let listed = list(&mut ws).await;
    let task = &listed["tasks"][0];
    assert_eq!((task["name"].clone(), task["profile"].clone(), task["tools"].clone(), task["enabled"].clone()), (json!("Morning"), json!("folder"), json!("Read,Grep,Glob"), json!(true)));
    assert!(task["next_due"].as_i64().unwrap() > 0);

    // A web task has no folder.
    let web = ask(&mut ws, json!({"op": "sched_save", "rid": 3, "name": "News", "prompt": "Top story?", "profile": "web", "cwd": "", "kind": "every", "every_min": 60, "domains": ["News.Example.com."]})).await;
    assert_eq!(web["ok"], json!(true), "{web}");
    assert_eq!(list(&mut ws).await["tasks"][1]["domains"], json!(["news.example.com"]), "kept in its normal form");
    let mixed = ask(&mut ws, json!({"op": "sched_save", "rid": 4, "name": "News", "prompt": "Top story?", "profile": "web", "cwd": fx.project.display().to_string(), "kind": "every", "every_min": 60, "domains": ["example.com"]})).await;
    assert_eq!(mixed["ok"], json!(false), "a web task with a project folder mixes files and the web");

    let mut edit = form(&fx, 5);
    edit["id"] = json!(id);
    edit["name"] = json!("Morning v2");
    assert_eq!(ask(&mut ws, edit).await["ok"], json!(true));
    assert_eq!(list(&mut ws).await["tasks"][0]["name"], json!("Morning v2"));

    assert_eq!(ask(&mut ws, json!({"op": "sched_enable", "rid": 6, "id": id, "on": false})).await["ok"], json!(true));
    assert_eq!(list(&mut ws).await["tasks"][0]["enabled"], json!(false));
    assert_eq!(ask(&mut ws, json!({"op": "sched_delete", "rid": 7, "id": id})).await["ok"], json!(true));
    assert_eq!(list(&mut ws).await["tasks"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn run_now_stores_a_result_the_page_can_open_and_opening_it_marks_it_seen() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let id = ask(&mut ws, form(&fx, 1)).await["id"].as_i64().unwrap();
    assert_eq!(ask(&mut ws, json!({"op": "sched_run_now", "rid": 2, "id": id})).await["ok"], json!(true));
    let announced = until_kind(&mut ws, "sched_run").await;
    assert_eq!((announced["data"]["status"].clone(), announced["data"]["name"].clone()), (json!("ok"), json!("Morning")));

    let listed = list(&mut ws).await;
    assert_eq!(listed["unseen"], json!(1));
    let run = &listed["runs"][0];
    assert!(run["result"].is_null(), "the list carries no result text");
    let run_id = run["id"].as_i64().unwrap();

    ws.send(Message::Text(json!({"op": "sched_run", "id": run_id}).to_string())).await.unwrap();
    let detail = until_kind(&mut ws, "sched_run_detail").await["data"]["run"].clone();
    assert_eq!(detail["result"], json!("fake report"));
    assert_eq!(detail["def"]["profile"], json!("folder"));
    assert_eq!(list(&mut ws).await["unseen"], json!(0));
}

#[tokio::test]
async fn stop_says_so_when_there_is_nothing_to_stop() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let id = ask(&mut ws, form(&fx, 1)).await["id"].as_i64().unwrap();
    let answer = ask(&mut ws, json!({"op": "sched_stop", "rid": 2, "id": id})).await;
    assert_eq!(answer["ok"], json!(false), "{answer}");
    assert!(list(&mut ws).await["running"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn switching_on_is_the_persons_act_and_survives_in_the_listing() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    assert_eq!(ask(&mut ws, json!({"op": "sched_switch", "rid": 1, "on": true})).await["ok"], json!(true));
    assert_eq!(list(&mut ws).await["on"], json!(true));
    assert_eq!(ask(&mut ws, json!({"op": "sched_switch", "rid": 2, "on": false})).await["ok"], json!(true));
    assert_eq!(list(&mut ws).await["on"], json!(false));
    // The page is told when something changes.
    ws.send(Message::Text(json!({"op": "sched_seen"}).to_string())).await.unwrap();
    until_kind(&mut ws, "sched_changed").await;
}

#[tokio::test]
async fn a_web_task_names_the_sites_it_may_open_and_only_host_names_are_taken() {
    let fx = fixture().await;
    let mut ws = good(&fx).await;
    let web = |rid: i64, domains: Value| {
        let mut op = json!({"op": "sched_save", "rid": rid, "name": "News", "prompt": "Top story?", "profile": "web", "cwd": "", "kind": "every", "every_min": 60});
        if !domains.is_null() {
            op["domains"] = domains;
        }
        op
    };
    for (what, domains) in [
        ("no list at all", Value::Null),
        ("an empty list", json!([])),
        ("a string instead of a list", json!("example.com")),
        ("an object", json!({"a": "example.com"})),
        ("a list with a number in it", json!(["example.com", 5])),
        ("a loopback address", json!(["127.0.0.1"])),
        ("a decimal address", json!(["2130706433"])),
        ("localhost", json!(["localhost"])),
        ("a private name", json!(["nas.local"])),
        ("a port", json!(["example.com:8080"])),
        ("an address with a path", json!(["https://example.com/a"])),
        ("a wildcard", json!(["*.example.com"])),
        ("too many sites", json!((0..21).map(|i| format!("s{i}.example.com")).collect::<Vec<_>>())),
    ] {
        let answer = ask(&mut ws, web(2, domains)).await;
        assert_eq!(answer["ok"], json!(false), "{what} must be refused: {answer}");
    }
    assert_eq!(list(&mut ws).await["tasks"].as_array().unwrap().len(), 0, "nothing was made by the refused forms");
    let folder = ask(&mut ws, json!({"op": "sched_save", "rid": 3, "name": "F", "prompt": "p", "profile": "folder", "cwd": fx.project.display().to_string(), "kind": "every", "every_min": 60, "domains": ["example.com"]})).await;
    assert_eq!(folder["ok"], json!(false), "a folder task opens no sites: {folder}");

    // A web task kept without sites (made by an older version, or edited by hand) is not switched on and does not run.
    let id = ask(&mut ws, web(4, json!(["example.com"]))).await["id"].as_i64().unwrap();
    let conn = rusqlite::Connection::open(fx.data.join("schedule.sqlite")).unwrap();
    conn.execute("UPDATE tasks SET domains = '[]', enabled = 0", []).unwrap();
    drop(conn);
    let on = ask(&mut ws, json!({"op": "sched_enable", "rid": 5, "id": id, "on": true})).await;
    assert_eq!(on["ok"], json!(false), "{on}");
    assert!(on["error"].as_str().unwrap().contains("sites"), "{on}");
    assert_eq!(list(&mut ws).await["tasks"][0]["enabled"], json!(false));
    let now = ask(&mut ws, json!({"op": "sched_run_now", "rid": 6, "id": id})).await;
    assert_eq!(now["ok"], json!(false), "{now}");
    // Giving it sites is the way back.
    assert_eq!(ask(&mut ws, { let mut op = web(7, json!(["example.org"])); op["id"] = json!(id); op }).await["ok"], json!(true));
    assert_eq!(ask(&mut ws, json!({"op": "sched_enable", "rid": 8, "id": id, "on": true})).await["ok"], json!(true));
}
