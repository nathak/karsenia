//! The terminal over the WebSocket: off unless started with the option, a shell in the chat's folder with a clean environment,
//! ended with its page, kept to the socket that opened it, and limited.
#![allow(clippy::result_large_err)]
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tenrec::server::{start, start_with, Config, Running};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-claude");

struct Fx {
    _dir: tempfile::TempDir,
    server: Running,
    project: std::path::PathBuf,
}

fn config(root: &std::path::Path) -> Config {
    let scenario = root.join("scenario.jsonl");
    std::fs::write(&scenario, "{\"$wait\":\"user\"}\n").unwrap();
    let wrapper = root.join("claude");
    std::fs::write(&wrapper, format!("#!/bin/sh\nFAKE_SCENARIO='{}' FAKE_LOG='{}' exec '{}' \"$@\"\n", scenario.display(), root.join("fake.log").display(), FAKE)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    Config { port: 0, data_dir: root.join("data"), bin: Some(wrapper.display().to_string()), mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }
}

async fn fixture(terminal: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let server = if terminal { start_with(config(dir.path()), true).await.unwrap() } else { start(config(dir.path())).await.unwrap() };
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

/// The next message of either kind, as text or bytes.
async fn next(ws: &mut Ws) -> Message {
    loop {
        let got = tokio::time::timeout(Duration::from_secs(10), ws.next()).await.expect("timed out").expect("closed").expect("error");
        if matches!(got, Message::Text(_) | Message::Binary(_)) {
            return got;
        }
    }
}

async fn send(ws: &mut Ws, op: Value) {
    ws.send(Message::Text(op.to_string())).await.unwrap();
}

async fn open_chat(fx: &Fx, ws: &mut Ws) -> String {
    send(ws, json!({"op": "new", "cwd": fx.project.display().to_string()})).await;
    loop {
        if let Message::Text(text) = next(ws).await {
            let v: Value = serde_json::from_str(&text).unwrap();
            if v["kind"] == "created" {
                return v["sid"].as_str().unwrap().to_owned();
            }
        }
    }
}

/// Opens a terminal and returns its answer (`term_opened` or `term_error`) with text of other kinds skipped.
async fn open_term(ws: &mut Ws, sid: &str, req: u64) -> Value {
    send(ws, json!({"op": "term_open", "sid": sid, "req": req, "cols": 100, "rows": 30})).await;
    loop {
        if let Message::Text(text) = next(ws).await {
            let v: Value = serde_json::from_str(&text).unwrap();
            if (v["kind"] == "term_opened" || v["kind"] == "term_error") && v["data"]["req"] == req {
                return v;
            }
        }
    }
}

struct Seen {
    text: String,
    bytes: usize,
    biggest: usize,
    exit: Option<i32>,
}

/// Takes in one binary frame for `tid` (text messages are skipped).
async fn step(ws: &mut Ws, tid: u32, seen: &mut Seen) {
    loop {
        if let Message::Binary(frame) = next(ws).await {
            assert_eq!(&frame[1..5], &tid.to_be_bytes(), "a frame of another terminal");
            match frame[0] {
                1 => {
                    seen.bytes += frame.len() - 5;
                    seen.biggest = seen.biggest.max(frame.len() - 5);
                    seen.text.push_str(&String::from_utf8_lossy(&frame[5..]));
                }
                2 => seen.exit = Some(i32::from_be_bytes(frame[5..9].try_into().unwrap())),
                other => panic!("unknown frame kind {other}"),
            }
            return;
        }
    }
}

/// Reads output frames for `tid` into `seen.text` until `until` appears, or the shell ends.
async fn read_until(ws: &mut Ws, tid: u32, until: &str, seen: &mut Seen) {
    while !seen.text.contains(until) && seen.exit.is_none() {
        step(ws, tid, seen).await;
    }
}

fn fresh() -> Seen {
    Seen { text: String::new(), bytes: 0, biggest: 0, exit: None }
}

async fn run(ws: &mut Ws, tid: u32, line: &str, until: &str) -> Seen {
    send(ws, json!({"op": "term_input", "tid": tid, "data": format!("{line}\n")})).await;
    let mut seen = fresh();
    read_until(ws, tid, until, &mut seen).await;
    seen
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only asks whether the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

async fn gone(pid: i32) -> bool {
    tokio::time::timeout(Duration::from_secs(8), async {
        while alive(pid) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .is_ok()
}

/// The number printed after `tag` on a line of its own (the typed line, echoed, has `$!` there and is not a number).
fn number_after(text: &str, tag: &str) -> Option<i32> {
    text.lines().find_map(|line| line.trim().strip_prefix(tag).and_then(|r| r.trim().parse().ok()))
}


/// Reads on until a line `PID<number>` has come (the typed line, echoed, shows `PID$!` and does not count).
async fn pid_of(ws: &mut Ws, tid: u32, mut seen: Seen) -> i32 {
    loop {
        if let Some(pid) = number_after(&seen.text, "PID") {
            return pid;
        }
        assert!(seen.exit.is_none(), "the shell ended before it printed a pid");
        step(ws, tid, &mut seen).await;
    }
}

#[tokio::test]
async fn without_the_start_option_there_is_no_terminal() {
    let fx = fixture(false).await;
    let mut ws = good(&fx).await;
    let sid = open_chat(&fx, &mut ws).await;
    let answer = open_term(&mut ws, &sid, 1).await;
    assert_eq!(answer["kind"], "term_error");
    assert!(answer["data"]["message"].as_str().unwrap().contains("--terminal"));
}

#[tokio::test]
async fn a_shell_starts_in_the_chats_folder_with_a_clean_environment() {
    // A key in the app's own environment must not reach the shell.
    std::env::set_var("OPENROUTER_API_KEY", "sk-test-not-for-the-shell");
    std::env::set_var("TENREC_SECRET_FOR_TEST", "hidden");
    let fx = fixture(true).await;
    let mut ws = good(&fx).await;
    let sid = open_chat(&fx, &mut ws).await;
    let opened = open_term(&mut ws, &sid, 1).await;
    assert_eq!(opened["kind"], "term_opened", "{opened}");
    let tid = opened["data"]["tid"].as_u64().unwrap() as u32;

    let seen = run(&mut ws, tid, "pwd; echo SIZE$(stty size) END$((1+1))", "END2").await;
    let project = fx.project.canonicalize().unwrap().display().to_string();
    assert!(seen.text.contains(&format!("{project}\r\n")), "{}", seen.text);
    assert!(seen.text.contains("SIZE30 100"), "{}", seen.text);

    // Resize, then ask again.
    send(&mut ws, json!({"op": "term_resize", "tid": tid, "cols": 132, "rows": 40})).await;
    let seen = run(&mut ws, tid, "echo SIZE$(stty size) END$((1+2))", "END3").await;
    assert!(seen.text.contains("SIZE40 132"), "{}", seen.text);

    let seen = run(&mut ws, tid, "env | cut -d= -f1 | sort | tr '\\n' ' '; echo; echo END$((2+2))", "END4").await;
    assert!(!seen.text.contains("OPENROUTER"), "{}", seen.text);
    assert!(!seen.text.contains("TENREC_"), "{}", seen.text);
    assert!(seen.text.contains("PATH") && seen.text.contains("TERM"), "{}", seen.text);
    let seen = run(&mut ws, tid, "echo \"[$OPENROUTER_API_KEY$TENREC_SECRET_FOR_TEST]\" END$((2+3))", "END5").await;
    assert!(seen.text.contains("[]"), "{}", seen.text);
}

#[tokio::test]
async fn a_close_or_a_closed_page_ends_the_shell_and_what_it_started() {
    let fx = fixture(true).await;
    let mut ws = good(&fx).await;
    let sid = open_chat(&fx, &mut ws).await;

    // term_close
    let tid = open_term(&mut ws, &sid, 1).await["data"]["tid"].as_u64().unwrap() as u32;
    let seen = run(&mut ws, tid, "sleep 1000 & echo PID$!", "PID").await;
    let pid = pid_of(&mut ws, tid, seen).await;
    assert!(alive(pid));
    send(&mut ws, json!({"op": "term_close", "tid": tid})).await;
    let mut tail = fresh();
    read_until(&mut ws, tid, "never appears", &mut tail).await;
    assert!(tail.exit.is_some(), "the end of the shell is told");
    assert!(gone(pid).await, "the background job outlived term_close");

    // a page that goes away
    let tid = open_term(&mut ws, &sid, 2).await["data"]["tid"].as_u64().unwrap() as u32;
    let seen = run(&mut ws, tid, "sleep 1000 & echo PID$!", "PID").await;
    let pid = pid_of(&mut ws, tid, seen).await;
    assert!(alive(pid));
    drop(ws);
    assert!(gone(pid).await, "the background job outlived the page");
}

#[tokio::test]
async fn a_close_ends_the_shell_even_while_its_input_is_backed_up() {
    let fx = fixture(true).await;
    let mut ws = good(&fx).await;
    let sid = open_chat(&fx, &mut ws).await;
    let tid = open_term(&mut ws, &sid, 1).await["data"]["tid"].as_u64().unwrap() as u32;
    // A program that reads no input: the typed text piles up on the host side until it holds back more.
    let seen = run(&mut ws, tid, "echo PID$$; exec sleep 1000", "PID").await;
    let pid = pid_of(&mut ws, tid, seen).await;
    let chunk = "a".repeat(3000);
    for _ in 0..40 {
        send(&mut ws, json!({"op": "term_input", "tid": tid, "data": format!("{chunk}\n")})).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    send(&mut ws, json!({"op": "term_close", "tid": tid})).await;
    assert!(gone(pid).await, "the shell outlived term_close while its input was backed up");
}

#[tokio::test]
async fn what_a_shell_printed_last_arrives_before_its_end_is_told() {
    let fx = fixture(true).await;
    let mut ws = good(&fx).await;
    let sid = open_chat(&fx, &mut ws).await;
    let tid = open_term(&mut ws, &sid, 1).await["data"]["tid"].as_u64().unwrap() as u32;
    // The typed line is echoed with `$((40+2))` in it; only the shell's own output says LAST42.
    send(&mut ws, json!({"op": "term_input", "tid": tid, "data": "echo LAST$((40+2)); exit\n"})).await;
    let mut seen = fresh();
    read_until(&mut ws, tid, "never appears", &mut seen).await;
    assert!(seen.exit.is_some(), "the end of the shell is told");
    assert!(seen.text.contains("LAST42"), "the last output was lost: {:?}", seen.text);
}

#[tokio::test]
async fn another_page_cannot_use_or_hear_a_terminal() {
    let fx = fixture(true).await;
    let mut first = good(&fx).await;
    let sid = open_chat(&fx, &mut first).await;
    let tid = open_term(&mut first, &sid, 1).await["data"]["tid"].as_u64().unwrap() as u32;
    let mut second = good(&fx).await;

    send(&mut second, json!({"op": "term_input", "tid": tid, "data": "echo STOLEN$((1+1))\n"})).await;
    send(&mut second, json!({"op": "term_close", "tid": tid})).await;
    let mut refused = 0;
    while refused < 2 {
        if let Message::Text(text) = next(&mut second).await {
            let v: Value = serde_json::from_str(&text).unwrap();
            if v["kind"] == "term_error" {
                assert_eq!(v["data"]["message"], "no such terminal");
                refused += 1;
            }
        }
    }
    // The first page's shell is untouched and is the only one that hears it.
    let seen = run(&mut first, tid, "echo MINE$((1+1))", "MINE2\r").await;
    assert!(!seen.text.contains("STOLEN2"), "{}", seen.text);
    send(&mut second, json!({"op": "list"})).await;
    loop {
        match tokio::time::timeout(Duration::from_secs(5), second.next()).await {
            Ok(Some(Ok(Message::Binary(_)))) => panic!("the second page heard terminal output"),
            Ok(Some(Ok(Message::Text(text)))) if text.contains("\"hello\"") && text.contains("sessions") => {
                if serde_json::from_str::<Value>(&text).unwrap()["kind"] == "hello" {
                    break;
                }
            }
            Ok(Some(_)) => {}
            _ => break,
        }
    }
}

#[tokio::test]
async fn places_are_limited_and_a_data_folder_chat_is_refused() {
    let fx = fixture(true).await;
    let mut ws = good(&fx).await;
    let sid = open_chat(&fx, &mut ws).await;
    assert_eq!(open_term(&mut ws, &sid, 1).await["kind"], "term_opened");
    assert_eq!(open_term(&mut ws, &sid, 2).await["kind"], "term_opened");
    let third = open_term(&mut ws, &sid, 3).await;
    assert_eq!(third["kind"], "term_error", "a page may have two");

    let data = fx._dir.path().join("data");
    send(&mut ws, json!({"op": "new", "cwd": data.display().to_string()})).await;
    let in_data = loop {
        if let Message::Text(text) = next(&mut ws).await {
            let v: Value = serde_json::from_str(&text).unwrap();
            if v["kind"] == "created" {
                break v["sid"].as_str().unwrap().to_owned();
            }
        }
    };
    let refused = open_term(&mut ws, &in_data, 4).await;
    assert_eq!(refused["kind"], "term_error");
    assert_eq!(open_term(&mut ws, "nope", 5).await["kind"], "term_error");
}

#[tokio::test]
async fn a_lot_of_output_arrives_whole_in_small_frames() {
    let fx = fixture(true).await;
    let mut ws = good(&fx).await;
    let sid = open_chat(&fx, &mut ws).await;
    let tid = open_term(&mut ws, &sid, 1).await["data"]["tid"].as_u64().unwrap() as u32;
    // Without the echo, so the count is only the output (the typed line holds no Q itself, in case it is echoed before `stty` acts).
    run(&mut ws, tid, "stty -echo; echo READY$((1+1))", "READY2").await;
    let seen = run(&mut ws, tid, "head -c 1000000 /dev/zero | tr '\\0' '\\121'; echo; echo DONE$((1+2))", "DONE3").await;
    // The longest run of Q: the prompt shows the temporary folder's random name, which may hold a Q of its own.
    let longest = seen.text.split(|c| c != 'Q').map(str::len).max().unwrap_or(0);
    assert_eq!(longest, 1_000_000);
    assert!(seen.biggest <= 16 * 1024, "a frame of {} bytes", seen.biggest);
    assert!(seen.bytes >= 1_000_000);
}

#[tokio::test]
async fn the_terminal_needs_the_token_and_the_right_origin() {
    let fx = fixture(true).await;
    let port = fx.server.addr.port();
    let mut request = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
    request.headers_mut().insert("Origin", HeaderValue::from_str(&format!("http://127.0.0.1:{port}")).unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    send(&mut ws, json!({"op": "term_open", "sid": "x", "req": 1})).await;
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return true,
                Some(Ok(Message::Text(_))) | Some(Ok(Message::Binary(_))) => return false,
                _ => {}
            }
        }
    })
    .await;
    assert_eq!(ended, Ok(true), "an op in place of the token ends the socket without an answer");

    let mut request = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
    request.headers_mut().insert("Origin", HeaderValue::from_static("http://evil.example"));
    assert!(tokio_tungstenite::connect_async(request).await.is_err(), "a page from another origin is refused");
}
