//! The live view's server: who may ask, what it answers, and the feed of a chat as a browser reads it.
use std::time::Duration;

use serde_json::Value;
use tenrec::live::server::{start, LiveServer};
use tenrec::live::Prompts;
use tenrec::server::{start_core, Config};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");
const KEY: &str = "sk-ant-q1w2e3r4t5y6u7i8o9p0q1w2e3r4t5y6u7i8o9p0";

struct Env {
    _dir: tempfile::TempDir,
    registry: std::sync::Arc<tenrec::registry::Registry>,
    project: std::path::PathBuf,
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let scenario = [
        r#"{"$wait":"user"}"#,
        r#"{"type":"system","subtype":"init","session_id":"748aa02c-6299-4224-8d60-1131a940bf14"}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"git push origin main"}}],"usage":{"input_tokens":10,"cache_read_input_tokens":90,"output_tokens":5}}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"pushed","is_error":false}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":10,"total_cost_usd":0.01,"usage":{"input_tokens":10,"output_tokens":5},"modelUsage":{"m":{"inputTokens":10,"outputTokens":5,"contextWindow":1000}}}"#,
        r#"{"$wait":"user"}"#,
    ]
    .join("\n");
    std::fs::write(root.join("scenario.jsonl"), scenario).unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' exec '{}' \"$@\"\n", root.join("scenario.jsonl").display(), FAKE)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let project = root.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let registry = start_core(Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).unwrap();
    Env { _dir: dir, registry, project }
}

/// One request, read to the end: (status, the head in lower case, the body).
async fn http(server: &LiveServer, method: &str, path: &str, headers: &[(&str, &str)]) -> (u16, String, String) {
    let mut stream = TcpStream::connect(server.addr).await.unwrap();
    let mut request = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\n");
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
        request.push_str(&format!("Host: 127.0.0.1:{}\r\n", server.addr.port()));
    }
    for (k, v) in headers {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    request.push_str("Content-Length: 0\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut raw)).await.unwrap().unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap();
    (status, head.to_lowercase(), body.to_owned())
}

fn bearer(server: &LiveServer) -> String {
    format!("Bearer {}", server.token())
}

/// A feed read until `event: ready` (the newest items are all in by then).
async fn read_feed(server: &LiveServer, sid: &str) -> String {
    let mut stream = TcpStream::connect(server.addr).await.unwrap();
    let request = format!("GET /api/stream?sid={sid} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: {}\r\nConnection: close\r\n\r\n", server.addr.port(), bearer(server));
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut seen = String::new();
    let mut buf = [0u8; 4096];
    let started = std::time::Instant::now();
    while !seen.contains("event: ready") {
        assert!(started.elapsed() < Duration::from_secs(8), "no ready: {seen}");
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf)).await.unwrap().unwrap();
        assert!(n > 0, "the feed ended: {seen}");
        seen.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    seen
}

fn items(feed: &str) -> Vec<Value> {
    feed.lines().filter_map(|l| l.strip_prefix("data: ")).filter_map(|d| serde_json::from_str::<Value>(d).ok()).filter(|v| v.get("t").is_some()).collect()
}

async fn run_one_turn(env: &Env, text: &str) -> String {
    let chat = env.registry.create_in(env.project.to_str().unwrap(), None, None, false, None).unwrap();
    let (_, mut rx) = chat.subscribe(0).await.unwrap();
    chat.send(text.into(), vec![]).await.unwrap();
    loop {
        let got = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.unwrap().unwrap();
        if got.kind == "claude" && got.data["type"] == "result" {
            break;
        }
    }
    chat.sid.clone()
}

#[tokio::test]
async fn only_its_own_page_with_its_token_may_ask_and_nothing_can_be_written() {
    let e = env().await;
    let server = start(e.registry.clone(), 0, Prompts::Full).await.unwrap();
    let auth = bearer(&server);

    let (status, head, body) = http(&server, "GET", "/", &[]).await;
    assert_eq!(status, 200);
    assert!(body.contains("<title>"), "the page is served without a token (it holds no data)");
    assert!(head.contains("content-security-policy") && head.contains("nosniff") && head.contains("no-referrer") && head.contains("no-store"), "{head}");
    assert!(!head.contains("access-control-allow"), "{head}");

    assert_eq!(http(&server, "GET", "/api/chats", &[]).await.0, 401, "no token");
    assert_eq!(http(&server, "GET", &format!("/api/chats?token={}", server.token()), &[]).await.0, 401, "a token in the address is not read");
    assert_eq!(http(&server, "GET", "/api/chats", &[("Authorization", "Bearer wrong")]).await.0, 401);
    let (status, _, body) = http(&server, "GET", "/api/chats", &[("Authorization", &auth)]).await;
    assert_eq!(status, 200);
    assert!(serde_json::from_str::<Value>(&body).unwrap().is_array());
    assert_eq!(http(&server, "GET", "/api/info", &[("Authorization", &auth)]).await.0, 200);

    assert_eq!(http(&server, "GET", "/", &[("Host", "evil.test:80")]).await.0, 421, "another name for this address (DNS rebinding)");
    assert_eq!(http(&server, "GET", "/", &[("Host", &format!("127.0.0.1:{}", server.addr.port() + 1))]).await.0, 421);
    assert_eq!(http(&server, "GET", "/api/chats", &[("Authorization", &auth), ("Origin", "http://evil.example")]).await.0, 403);
    assert_eq!(http(&server, "GET", "/api/chats", &[("Authorization", &auth), ("Sec-Fetch-Site", "cross-site")]).await.0, 403);
    assert_eq!(http(&server, "GET", "/api/chats", &[("Authorization", &auth), ("Origin", &format!("http://127.0.0.1:{}", server.addr.port()))]).await.0, 200, "its own origin is fine");

    assert_eq!(http(&server, "GET", "/api/chats", &[("Authorization", &auth), ("Origin", "null")]).await.0, 403, "a page with no origin (a sandboxed one) is refused");
    assert_eq!(http(&server, "OPTIONS", "/api/chats", &[("Access-Control-Request-Method", "GET"), ("Origin", "http://evil.example")]).await.0, 403, "no preflight is answered");
    assert_eq!(http(&server, "OPTIONS", "/api/chats", &[]).await.0, 401);
    assert_eq!(http(&server, "HEAD", "/api/chats", &[]).await.0, 401);
    assert_eq!(http(&server, "OPTIONS", "/api/chats", &[("Authorization", &auth)]).await.0, 405);
    for method in ["POST", "PUT", "DELETE", "PATCH"] {
        for path in ["/", "/api/chats", "/api/stream?sid=x", "/ws"] {
            let (status, _, _) = http(&server, method, path, &[("Authorization", &auth)]).await;
            assert_eq!(status, 405, "{method} {path}");
        }
    }
    for path in ["/ws", "/api/file", "/api/file?path=Cargo.toml", "/index.html", "/../Cargo.toml", "/data/x", "/api/nothing"] {
        let (status, _, body) = http(&server, "GET", path, &[("Authorization", &auth)]).await;
        assert_eq!(status, 404, "{path}");
        assert!(!body.contains("[package]"), "{path}");
    }
    assert_eq!(http(&server, "GET", "/api/stream?sid=nope", &[("Authorization", &auth)]).await.0, 404);
}

#[tokio::test]
async fn the_feed_of_a_chat_carries_its_prompt_its_call_with_the_guard_mark_and_the_end_of_the_turn_with_secrets_masked() {
    let e = env().await;
    let server = start(e.registry.clone(), 0, Prompts::Full).await.unwrap();
    let sid = run_one_turn(&e, &format!("push it, my key is {KEY}")).await;

    let feed = read_feed(&server, &sid).await;
    assert!(!feed.contains("q1w2e3r4t5"), "the key reached the wire: {feed}");
    let list = items(&feed);
    let kinds: Vec<&str> = list.iter().filter_map(|v| v["t"].as_str()).collect();
    assert_eq!(kinds, ["prompt", "state", "call", "call_end", "turn_end", "state"], "{feed}");
    let list: Vec<Value> = list.into_iter().filter(|v| v["t"] != "state").collect();
    assert!(list[0]["text"].as_str().unwrap().contains("push it"));
    let call = &list[1];
    assert_eq!((call["tool"].as_str(), call["category"].as_str()), (Some("Bash"), Some("external-send")), "{call}");
    assert_eq!(call["guard"]["derived"], true);
    assert_eq!(call["guard"]["policy"], "ask", "balanced asks before a push: {call}");
    assert_eq!((list[2]["status"].as_str(), list[2]["output"].as_str()), (Some("ok"), Some("pushed")));
    assert_eq!((list[3]["ok"].as_bool(), list[3]["context"]["window"].as_u64()), (Some(true), Some(1000)));
    assert!(list.iter().all(|v| v["turn"] == 1 && v["sid"] == sid));

    // The chats are a tab each.
    let (_, _, body) = http(&server, "GET", "/api/chats", &[("Authorization", &bearer(&server))]).await;
    let tabs: Value = serde_json::from_str(&body).unwrap();
    assert!(tabs.as_array().unwrap().iter().any(|t| t["sid"] == sid.as_str() && t["project"] == "p"), "{body}");
    assert!(!body.contains(e.project.parent().unwrap().to_str().unwrap()), "only the last part of the folder is given: {body}");
}

#[tokio::test]
async fn the_short_mode_cuts_a_prompt_for_the_pages_that_connect_after_it_is_set() {
    let e = env().await;
    let server = start(e.registry.clone(), 0, Prompts::Full).await.unwrap();
    let sid = run_one_turn(&e, &"long ".repeat(100)).await;
    let full = items(&read_feed(&server, &sid).await);
    assert!(full[0]["text"].as_str().unwrap().chars().count() > 400);
    server.set_prompts(Prompts::Short);
    assert_eq!(server.prompts(), Prompts::Short);
    let short = items(&read_feed(&server, &sid).await);
    assert_eq!((short[0]["text"].as_str().unwrap().chars().count(), short[0]["truncated"].as_bool()), (120, Some(true)));
}

#[tokio::test]
async fn a_taken_port_gives_the_next_one_and_a_stopped_server_closes_its_port() {
    let e = env().await;
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let taken = held.local_addr().unwrap().port();
    let server = start(e.registry.clone(), taken, Prompts::Full).await.unwrap();
    assert!(server.addr.port() > taken && server.addr.port() < taken.saturating_add(20), "{} vs {taken}", server.addr.port());
    let addr = server.addr;
    assert!(TcpStream::connect(addr).await.is_ok());
    server.stop();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(TcpStream::connect(addr).await.is_err(), "the port is closed after stop");
}

#[tokio::test]
async fn an_open_feed_ends_when_the_server_stops() {
    let e = env().await;
    let server = start(e.registry.clone(), 0, Prompts::Full).await.unwrap();
    let sid = run_one_turn(&e, "go").await;
    let mut stream = TcpStream::connect(server.addr).await.unwrap();
    let request = format!("GET /api/stream?sid={sid} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: {}\r\nConnection: close\r\n\r\n", server.addr.port(), bearer(&server));
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = [0u8; 8192];
    let mut seen = String::new();
    while !seen.contains("event: ready") {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf)).await.unwrap().unwrap();
        seen.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    server.stop();
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the open feed did not end");
}
